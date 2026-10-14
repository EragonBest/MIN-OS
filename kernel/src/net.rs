//! Rete di MIN-OS: driver e1000 + stack TCP/IP smoltcp (lo stesso usato da Redox OS).
//!
//! All'avvio cerca una scheda di rete, chiede un indirizzo con DHCP e configura i DNS.
//! Dal Terminale: `ip`, `ping <host>`, `dns <host>`, `get http://...`.

use crate::arch::{now_ms, rdtsc};
use crate::e1000::E1000;
use crate::i18n_desk::Dk;
use crate::pci;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::dns::{self, GetQueryResultError, QueryHandle};
use smoltcp::socket::{dhcpv4, icmp, tcp};
use smoltcp::time::Instant;
use smoltcp::wire::{DnsQueryType, EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};

const IDENT: u16 = 0x4D4E;

// ------------------------------------------------------------------ smoltcp <-> e1000

pub struct Rx(Vec<u8>);
pub struct Tx<'a>(&'a mut E1000);

impl RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl<'a> TxToken for Tx<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut out = None;
        self.0.send(len, |buf| out = Some(f(buf)));
        out.unwrap()
    }
}

impl Device for E1000 {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;

    fn receive(&mut self, _t: Instant) -> Option<(Rx, Tx<'_>)> {
        let p = self.recv()?;
        Some((Rx(p), Tx(self)))
    }

    fn transmit(&mut self, _t: Instant) -> Option<Tx<'_>> {
        if self.can_send() {
            Some(Tx(self))
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ethernet;
        c.max_transmission_unit = 1514;
        // smoltcp limita la finestra TCP a burst * MSS: con l anello da 256 pacchetti ne reggiamo molti
        c.max_burst_size = Some(128);
        c
    }
}

// ------------------------------------------------------------------ stato della rete

#[derive(Clone)]
pub struct Info {
    pub present: bool,
    pub name: &'static str,
    pub mac: [u8; 6],
    pub link: bool,
    pub ip: Option<Ipv4Cidr>,
    pub gateway: Option<Ipv4Address>,
    pub dns: Vec<Ipv4Address>,
    pub rx: u64,
    pub tx: u64,
}

enum Target {
    Resolving(String, Option<QueryHandle>),
    Ready(Ipv4Address),
}

enum Task {
    Idle,
    Dns { host: String, q: Option<QueryHandle>, start: u64 },
    Ping { target: Target, sent: u16, recv: u16, waiting: Option<(u16, u64)>, last: u64, start: u64 },
    Get { target: Target, host: String, path: String, port: u16, stage: u8, data: Vec<u8>, start: u64, app: bool, tls: bool },
}

struct Net {
    dev: E1000,
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: SocketHandle,
    dns: SocketHandle,
    icmp: SocketHandle,
    tcp: Option<SocketHandle>,
    ip: Option<Ipv4Cidr>,
    gateway: Option<Ipv4Address>,
    dns_servers: Vec<Ipv4Address>,
    task: Task,
    out: Vec<String>,
    dk: &'static Dk,
    port: u16,
    fetch: Fetch,
    /// chi ha chiesto il download (1 = browser, 2 = aggiornamenti)
    owner: u8,
}

/// Risposta HTTP scaricata per l'app Internet.
pub struct Resp {
    pub status: u16,
    pub ctype: String,
    pub location: String,
    pub body: Vec<u8>,
    pub truncated: bool,
}

/// Stato del download dell'app Internet.
pub enum Fetch {
    Idle,
    Busy(usize),
    Done(Resp),
    /// errore; true se e' fallita la connessione sicura (HTTPS)
    Failed(String, bool),
}

const APP_LIMIT: usize = 16 * 1024 * 1024;

static mut NET: Option<Net> = None;

fn net() -> Option<&'static mut Net> {
    unsafe { (*core::ptr::addr_of_mut!(NET)).as_mut() }
}

fn now() -> Instant {
    Instant::from_millis(now_ms() as i64)
}

pub fn init(devs: &[pci::Dev]) -> bool {
    let Some(nic) = devs.iter().filter(|d| d.vendor == 0x8086 && d.class == 0x02).find_map(E1000::new) else {
        crate::log!("rete: nessuna scheda compatibile");
        return false;
    };
    let mut dev = nic;
    let mut cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(dev.mac)));
    cfg.random_seed = rdtsc();
    let iface = Interface::new(cfg, &mut dev, now());
    let mut sockets = SocketSet::new(Vec::new());
    let dhcp = sockets.add(dhcpv4::Socket::new());
    let dns = sockets.add(dns::Socket::new(&[], Vec::new()));
    let mut ic = icmp::Socket::new(
        icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 8], vec![0; 4096]),
        icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 8], vec![0; 4096]),
    );
    let _ = ic.bind(icmp::Endpoint::Ident(IDENT));
    let icmp = sockets.add(ic);
    unsafe {
        *core::ptr::addr_of_mut!(NET) = Some(Net {
            dev,
            iface,
            sockets,
            dhcp,
            dns,
            icmp,
            tcp: None,
            ip: None,
            gateway: None,
            dns_servers: Vec::new(),
            task: Task::Idle,
            fetch: Fetch::Idle,
            owner: 0,
            out: Vec::new(),
            dk: crate::i18n::Lang::It.dk(),
            port: 49152 + (rdtsc() % 8000) as u16,
        })
    };
    true
}

pub fn info() -> Info {
    match net() {
        Some(n) => Info {
            present: true,
            name: n.dev.name,
            mac: n.dev.mac,
            link: n.dev.link(),
            ip: n.ip,
            gateway: n.gateway,
            dns: n.dns_servers.clone(),
            rx: n.dev.rx_count,
            tx: n.dev.tx_count,
        },
        None => Info { present: false, name: "", mac: [0; 6], link: false, ip: None, gateway: None, dns: Vec::new(), rx: 0, tx: 0 },
    }
}

/// Righe da mostrare nel Terminale (prodotte dai comandi di rete in corso).
pub fn take_output() -> Vec<String> {
    net().map(|n| core::mem::take(&mut n.out)).unwrap_or_default()
}

/// Avvia lo scaricamento di un indirizzo http:// per l'app Internet.
pub fn fetch(url: &str, dk: &'static Dk, owner: u8) -> Result<(), String> {
    let Some(n) = net() else { return Err(String::from(dk.no_adapter)) };
    n.dk = dk;
    let (tls, host, port, path) = split_url(url);
    if host.is_empty() {
        return Err(String::from(dk.page_error));
    }
    // un download nuovo sostituisce quello in corso
    n.drop_task();
    n.task = Task::Get { target: target(&host), host, path, port, stage: 0, data: Vec::new(), start: now_ms(), app: true, tls };
    n.fetch = Fetch::Busy(0);
    n.owner = owner;
    Ok(())
}

/// Chi possiede il download in corso.
pub fn fetch_owner() -> u8 {
    net().map(|n| n.owner).unwrap_or(0)
}

/// Annulla il download (solo se e' di `owner`).
pub fn fetch_cancel(owner: u8) {
    if let Some(n) = net() {
        if n.owner != owner {
            return;
        }
        if let Task::Get { app: true, .. } = n.task {
            n.drop_task();
        }
        n.fetch = Fetch::Idle;
    }
}

/// Byte ricevuti finora (None se nessun download in corso).
pub fn fetch_progress(owner: u8) -> Option<usize> {
    match net().filter(|n| n.owner == owner).map(|n| &n.fetch) {
        Some(Fetch::Busy(b)) => Some(*b),
        _ => None,
    }
}

/// Prende il risultato finale del download (se pronto).
pub fn fetch_take(owner: u8) -> Option<Result<Resp, (String, bool)>> {
    let n = net().filter(|n| n.owner == owner)?;
    match core::mem::replace(&mut n.fetch, Fetch::Idle) {
        Fetch::Done(r) => Some(Ok(r)),
        Fetch::Failed(e, tls) => Some(Err((e, tls))),
        other => {
            n.fetch = other;
            None
        }
    }
}

/// Divide "http(s)://host:porta/percorso" in (https?, host, porta, percorso).
pub fn split_url(url: &str) -> (bool, String, u16, String) {
    let tls = url.len() >= 8 && url[..8].eq_ignore_ascii_case("https://");
    let u = if tls { &url[8..] } else if url.len() >= 7 && url[..7].eq_ignore_ascii_case("http://") { &url[7..] } else { url };
    let (hostport, path) = match u.find(|c| c == '/' || c == '?' || c == '#') {
        Some(i) if u.as_bytes()[i] == b'/' => (&u[..i], String::from(&u[i..])),
        Some(i) if u.as_bytes()[i] == b'?' => (&u[..i], format!("/{}", &u[i..])),
        Some(i) => (&u[..i], String::from("/")),
        None => (u, String::from("/")),
    };
    let path = match path.find('#') {
        Some(i) => String::from(&path[..i]),
        None => path,
    };
    let def = if tls { 443 } else { 80 };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(def)),
        None => (hostport, def),
    };
    (tls, host.to_ascii_lowercase(), port, path.replace(' ', "%20"))
}

pub fn busy() -> bool {
    net().map(|n| !matches!(n.task, Task::Idle)).unwrap_or(false)
}

/// Fa avanzare la rete. Ritorna true se e' cambiato qualcosa di visibile.
pub fn poll() -> bool {
    let Some(n) = net() else { return false };
    let t = now();
    n.iface.poll(t, &mut n.dev, &mut n.sockets);
    let mut changed = false;

    // DHCP
    let ev = n.sockets.get_mut::<dhcpv4::Socket>(n.dhcp).poll();
    match ev {
        Some(dhcpv4::Event::Configured(c)) => {
            let (addr, router) = (c.address, c.router);
            let servers: Vec<Ipv4Address> = c.dns_servers.iter().copied().collect();
            n.iface.update_ip_addrs(|a| {
                a.clear();
                let _ = a.push(IpCidr::Ipv4(addr));
            });
            match router {
                Some(r) => {
                    let _ = n.iface.routes_mut().add_default_ipv4_route(r);
                }
                None => {
                    n.iface.routes_mut().remove_default_ipv4_route();
                }
            }
            let dns_addrs: Vec<IpAddress> = servers.iter().map(|s| IpAddress::Ipv4(*s)).collect();
            n.sockets.get_mut::<dns::Socket>(n.dns).update_servers(&dns_addrs);
            crate::log!("DHCP: {} gateway {:?} DNS {:?}", addr, router, servers);
            n.ip = Some(addr);
            n.gateway = router;
            n.dns_servers = servers;
            changed = true;
        }
        Some(dhcpv4::Event::Deconfigured) => {
            n.iface.update_ip_addrs(|a| a.clear());
            n.iface.routes_mut().remove_default_ipv4_route();
            n.ip = None;
            n.gateway = None;
            changed = true;
        }
        None => {}
    }

    let before = n.out.len();
    n.step_task();
    let changed = changed || n.out.len() != before;
    // da qui in poi `n` non si usa piu': tls_pump riprende la rete da sola
    tls_pump() || changed
}

// ------------------------------------------------------------------ comandi

/// Esegue un comando di rete del Terminale. Ritorna false se il comando non e' di rete.
pub fn command(name: &str, arg: &str, dk: &'static Dk) -> bool {
    if !matches!(name, "ip" | "ipconfig" | "ifconfig" | "ping" | "dns" | "nslookup" | "get" | "curl" | "wget") {
        return false;
    }
    let Some(n) = net() else {
        crate::net::push_static(dk.no_adapter);
        return true;
    };
    n.dk = dk;
    if !matches!(n.task, Task::Idle) {
        n.out.push(String::from(dk.net_busy));
        return true;
    }
    let start = now_ms();
    match name {
        "ip" | "ipconfig" | "ifconfig" => {
            let i = info();
            n.out.push(format!("{}: {}", dk.adapter, i.name));
            n.out.push(format!("{}: {}", dk.mac_addr, fmt_mac(&i.mac)));
            n.out.push(format!("{}: {}", dk.ip_addr, i.ip.map(|c| c.to_string()).unwrap_or_else(|| String::from("-"))));
            n.out.push(format!("{}: {}", dk.gateway, i.gateway.map(|g| g.to_string()).unwrap_or_else(|| String::from("-"))));
            n.out.push(format!("{}: {}", dk.dns_srv, join_ips(&i.dns)));
        }
        _ if n.ip.is_none() => n.out.push(String::from(dk.waiting_dhcp)),
        "ping" => {
            if arg.is_empty() {
                n.out.push(String::from("ping <host>   (es. ping 10.0.2.2)"));
            } else {
                n.task = Task::Ping { target: target(arg), sent: 0, recv: 0, waiting: None, last: 0, start };
            }
        }
        "dns" | "nslookup" => {
            // "dns <host> @<server>" usa un server DNS diverso (come nslookup)
            // "dns <host> <server>" oppure "dns <host> @<server>" (come nslookup)
            let (host, server) = match arg.split_once(' ') {
                Some((h, srv)) => (h.trim(), srv.trim().trim_start_matches('@').parse::<Ipv4Address>().ok()),
                None => (arg, None),
            };
            if let Some(srv) = server {
                n.dns_servers = vec![srv];
                n.sockets.get_mut::<dns::Socket>(n.dns).update_servers(&[IpAddress::Ipv4(srv)]);
                n.out.push(format!("{}: {}", dk.dns_srv, srv));
            }
            if host.is_empty() {
                n.out.push(String::from("dns <host> [server]"));
            } else {
                n.task = Task::Dns { host: String::from(host), q: None, start };
            }
        }
        _ => {
            if arg.is_empty() {
                n.out.push(String::from("get http(s)://host/percorso"));
            } else {
                let (tls, host, port, path) = split_url(arg);
                n.out.push(format!("{} {}:{} ...", dk.connecting, host, port));
                n.task = Task::Get { target: target(&host), host, path, port, stage: 0, data: Vec::new(), start, app: false, tls };
            }
        }
    }
    true
}

fn push_static(s: &str) {
    // nessuna scheda: l'uscita va comunque mostrata
    unsafe {
        (*core::ptr::addr_of_mut!(PENDING)).push(String::from(s));
    }
}

static mut PENDING: Vec<String> = Vec::new();

/// Messaggi prodotti quando la rete non esiste (nessuna scheda).
pub fn take_pending() -> Vec<String> {
    unsafe { core::mem::take(&mut *core::ptr::addr_of_mut!(PENDING)) }
}

fn target(host: &str) -> Target {
    match host.parse::<Ipv4Address>() {
        Ok(a) => Target::Ready(a),
        Err(_) => Target::Resolving(String::from(host), None),
    }
}

fn fmt_mac(m: &[u8; 6]) -> String {
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}

pub fn mac_string(m: &[u8; 6]) -> String {
    fmt_mac(m)
}

fn join_ips(v: &[Ipv4Address]) -> String {
    if v.is_empty() {
        return String::from("-");
    }
    v.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ")
}

enum Resolve {
    Pending,
    Done(Ipv4Address),
    Failed,
}

impl Net {
    /// Interrompe l'attivita' in corso liberando socket e richieste DNS.
    fn drop_task(&mut self) {
        let task = core::mem::replace(&mut self.task, Task::Idle);
        let q = match task {
            Task::Get { target: Target::Resolving(_, q), .. } | Task::Ping { target: Target::Resolving(_, q), .. } | Task::Dns { q, .. } => q,
            _ => None,
        };
        if let Some(h) = q {
            let _ = self.sockets.get_mut::<dns::Socket>(self.dns).cancel_query(h);
        }
        self.close_tcp();
    }

    fn resolve(&mut self, t: &mut Target) -> Resolve {
        match t {
            Target::Ready(a) => Resolve::Done(*a),
            Target::Resolving(host, q) => {
                let s = self.sockets.get_mut::<dns::Socket>(self.dns);
                match q {
                    None => match s.start_query(self.iface.context(), host, DnsQueryType::A) {
                        Ok(h) => {
                            *q = Some(h);
                            Resolve::Pending
                        }
                        Err(_) => Resolve::Failed,
                    },
                    Some(h) => match s.get_query_result(*h) {
                        Ok(addrs) => {
                            for a in addrs.iter() {
                                let IpAddress::Ipv4(v4) = a;
                                *t = Target::Ready(*v4);
                                return Resolve::Done(*v4);
                            }
                            Resolve::Failed
                        }
                        Err(GetQueryResultError::Pending) => Resolve::Pending,
                        Err(_) => Resolve::Failed,
                    },
                }
            }
        }
    }

    fn step_task(&mut self) {
        let dk = self.dk;
        let nowm = now_ms();
        let mut task = core::mem::replace(&mut self.task, Task::Idle);
        let keep = match &mut task {
            Task::Idle => false,
            Task::Dns { host, q, start } => {
                let mut tg = Target::Resolving(host.clone(), *q);
                match self.resolve(&mut tg) {
                    Resolve::Pending => {
                        if let Target::Resolving(_, nq) = tg {
                            *q = nq;
                        }
                        if nowm > *start + 8000 {
                            self.out.push(format!("{}: {}", dk.host_not_found, host));
                            false
                        } else {
                            true
                        }
                    }
                    Resolve::Done(a) => {
                        self.out.push(format!("{} -> {}", host, a));
                        false
                    }
                    Resolve::Failed => {
                        self.out.push(format!("{}: {}", dk.host_not_found, host));
                        false
                    }
                }
            }
            Task::Ping { target, sent, recv, waiting, last, start } => match self.resolve(target) {
                Resolve::Pending => nowm < *start + 8000 || {
                    self.out.push(String::from(dk.host_not_found));
                    false
                },
                Resolve::Failed => {
                    self.out.push(String::from(dk.host_not_found));
                    false
                }
                Resolve::Done(addr) => {
                    // risposte
                    let caps = ChecksumCapabilities::default();
                    let sock = self.sockets.get_mut::<icmp::Socket>(self.icmp);
                    while sock.can_recv() {
                        let Ok((payload, _)) = sock.recv() else { break };
                        let Ok(pkt) = Icmpv4Packet::new_checked(payload) else { continue };
                        if let Ok(Icmpv4Repr::EchoReply { ident, seq_no, .. }) = Icmpv4Repr::parse(&pkt, &caps) {
                            if let Some((s, t0)) = *waiting {
                                if ident == IDENT && seq_no == s {
                                    self.out.push(format!("{} {}: seq={} {}={} ms", dk.reply_from, addr, s, dk.time_word, nowm.saturating_sub(t0)));
                                    *recv += 1;
                                    *waiting = None;
                                }
                            }
                        }
                    }
                    if let Some((_, t0)) = *waiting {
                        if nowm > t0 + 2000 {
                            self.out.push(String::from(dk.timeout));
                            *waiting = None;
                        }
                    }
                    if waiting.is_none() && *sent < 4 && nowm >= *last + 1000 {
                        let seq = *sent + 1;
                        let data = [0x4Du8; 32];
                        let repr = Icmpv4Repr::EchoRequest { ident: IDENT, seq_no: seq, data: &data };
                        if let Ok(buf) = sock.send(repr.buffer_len(), IpAddress::Ipv4(addr)) {
                            let mut p = Icmpv4Packet::new_unchecked(buf);
                            repr.emit(&mut p, &caps);
                            *waiting = Some((seq, nowm));
                            *sent = seq;
                            *last = nowm;
                        }
                    }
                    if *sent >= 4 && waiting.is_none() {
                        self.out.push(format!("{}: {} {}, {} {}", addr, sent, dk.sent_word, recv, dk.recv_word));
                        false
                    } else {
                        true
                    }
                }
            },
            Task::Get { target, host, path, port, stage, data, start, app, tls } => {
                if nowm > *start + 20_000 {
                    self.close_tcp();
                    if *app {
                        if data.is_empty() {
                            self.fetch = Fetch::Failed(String::from(dk.timeout), false);
                        } else {
                            let d = core::mem::take(data);
                            self.finish_app(d, true);
                        }
                    } else {
                        self.out.push(String::from(dk.timeout));
                        if !data.is_empty() {
                            self.print_http(data);
                        }
                    }
                    false
                } else {
                    match self.resolve(target) {
                        Resolve::Pending => true,
                        Resolve::Failed => {
                            let m = format!("{}: {}", dk.host_not_found, host);
                            self.fail(*app, m);
                            false
                        }
                        // la parte cifrata (stadio 3+) la fa tls_pump(), fuori da questo metodo
                        Resolve::Done(_) if *tls && *stage >= 3 => true,
                        Resolve::Done(addr) => self.step_http(addr, host, path, *port, stage, data, start, *app, *tls),
                    }
                }
            }
        };
        if keep {
            self.task = task;
        }
    }

    fn close_tcp(&mut self) {
        tls_drop();
        if let Some(h) = self.tcp.take() {
            self.sockets.get_mut::<tcp::Socket>(h).abort();
            self.sockets.remove(h);
        }
    }

    fn fail(&mut self, app: bool, m: String) {
        if app {
            self.fetch = Fetch::Failed(m, false);
        } else {
            self.out.push(m);
        }
    }

    fn finish_app(&mut self, d: Vec<u8>, truncated: bool) {
        let split = d.windows(4).position(|w| w == b"\r\n\r\n");
        let (head, body) = match split {
            Some(i) => (String::from_utf8_lossy(&d[..i]).into_owned(), d[i + 4..].to_vec()),
            None => (String::from_utf8_lossy(&d).into_owned(), Vec::new()),
        };
        let mut lines = head.lines();
        let status = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).unwrap_or(0);
        let (mut ctype, mut location) = (String::new(), String::new());
        let mut chunked = false;
        for l in lines {
            if let Some((k, v)) = l.split_once(':') {
                let k = k.trim().to_ascii_lowercase();
                if k == "content-type" {
                    ctype = v.trim().to_ascii_lowercase();
                } else if k == "location" {
                    location = String::from(v.trim());
                } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                    chunked = true;
                }
            }
        }
        let body = if chunked { dechunk(&body) } else { body };
        if status == 0 {
            self.fetch = Fetch::Failed(String::from(self.dk.page_error), false);
        } else {
            self.fetch = Fetch::Done(Resp { status, ctype, location, body, truncated });
        }
    }

    fn step_http(&mut self, addr: Ipv4Address, host: &str, path: &str, port: u16, stage: &mut u8, data: &mut Vec<u8>, start: &mut u64, app: bool, tls: bool) -> bool {
        match *stage {
            0 => {
                self.close_tcp();
                // 65535: oltre servirebbe il "window scaling", che slirp e molti server non accettano
                let rx = 65535;
                let s = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; rx]), tcp::SocketBuffer::new(vec![0; 4096]));
                let h = self.sockets.add(s);
                self.tcp = Some(h);
                self.port = if self.port >= 65000 { 49152 } else { self.port + 1 };
                let local = self.port;
                let sock = self.sockets.get_mut::<tcp::Socket>(h);
                if sock.connect(self.iface.context(), (IpAddress::Ipv4(addr), port), local).is_err() {
                    self.fail(app, String::from(self.dk.conn_refused));
                    return false;
                }
                *stage = 1;
                true
            }
            1 => {
                let Some(h) = self.tcp else { return false };
                let sock = self.sockets.get_mut::<tcp::Socket>(h);
                if sock.may_send() && tls {
                    // connessione aperta: ora la stretta di mano TLS
                    *stage = 3;
                    true
                } else if sock.may_send() {
                    let req = http_request(host, path);
                    let _ = sock.send_slice(req.as_bytes());
                    *stage = 2;
                    true
                } else if !sock.is_open() {
                    self.close_tcp();
                    self.fail(app, String::from(self.dk.conn_refused));
                    false
                } else {
                    true
                }
            }
            _ => {
                let Some(h) = self.tcp else { return false };
                let sock = self.sockets.get_mut::<tcp::Socket>(h);
                let mut buf = [0u8; 8192];
                let limit = if app { APP_LIMIT } else { 512 * 1024 };
                let mut full = false;
                while sock.can_recv() {
                    match sock.recv_slice(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(k) => {
                            *start = now_ms();
                            if data.len() < limit {
                                data.extend_from_slice(&buf[..k]);
                            } else {
                                full = true;
                            }
                        }
                    }
                }
                if app {
                    self.fetch = Fetch::Busy(data.len());
                }
                if full && app {
                    sock.abort();
                    let d = core::mem::take(data);
                    self.close_tcp();
                    self.finish_app(d, true);
                    false
                } else if !sock.may_recv() || !sock.is_open() || response_complete(data) {
                    sock.close();
                    let d = core::mem::take(data);
                    self.close_tcp();
                    if app {
                        self.finish_app(d, false);
                    } else {
                        self.print_http(&d);
                    }
                    false
                } else {
                    true
                }
            }
        }
    }

    fn print_http(&mut self, data: &[u8]) {
        let fixed;
        let data = match data.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(he) if String::from_utf8_lossy(&data[..he]).to_ascii_lowercase().contains("transfer-encoding: chunked") => {
                let mut v = data[..he + 4].to_vec();
                v.extend(dechunk(&data[he + 4..]));
                fixed = v;
                &fixed[..]
            }
            _ => data,
        };
        let text = String::from_utf8_lossy(data);
        let (head, body) = match text.find("\r\n\r\n") {
            Some(i) => (&text[..i], &text[i + 4..]),
            None => (&text[..], ""),
        };
        let mut lines = head.lines();
        if let Some(status) = lines.next() {
            self.out.push(String::from(status.trim()));
        }
        let mut html = false;
        for l in lines {
            let low = l.to_ascii_lowercase();
            if low.starts_with("content-type:") || low.starts_with("server:") || low.starts_with("location:") {
                self.out.push(String::from(l.trim()));
                if low.contains("html") {
                    html = true;
                }
            }
        }
        self.out.push(String::new());
        let body_text = if html { strip_html(body) } else { String::from(body) };
        let mut shown = 0;
        for l in body_text.lines() {
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            let t: String = t.chars().take(160).collect();
            self.out.push(t);
            shown += 1;
            if shown >= 40 {
                self.out.push(String::from("..."));
                break;
            }
        }
        self.out.push(format!("— {} {}", data.len(), self.dk.bytes_word));
    }
}

/// Trasforma una pagina HTML in testo semplice da leggere nel Terminale.
fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    let mut skip = false;
    for c in s.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                let t = tag.to_ascii_lowercase();
                let name: String = t.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                if name == "script" || name == "style" {
                    skip = !t.starts_with('/');
                }
                if matches!(name.as_str(), "p" | "br" | "div" | "h1" | "h2" | "h3" | "li" | "tr" | "title" | "section" | "article") {
                    out.push('\n');
                }
                tag.clear();
            } else {
                tag.push(c);
            }
        } else if c == '<' {
            in_tag = true;
        } else if !skip {
            out.push(c);
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}


// ------------------------------------------------------------------ HTTP: richiesta e risposta

/// Nome con cui MIN-OS si presenta ai siti. "Lynx" fa avere a molti siti la versione
/// semplice delle pagine (senza JavaScript), l'unica che il nostro browser sa mostrare.
const USER_AGENT: &str = "Lynx/2.9.2 libwww-FM/2.14 SSL-MM/1.4.1 (MIN-OS)";

fn http_request(host: &str, path: &str) -> String {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: text/html, text/plain, */*;q=0.8\r\nAccept-Language: it, en;q=0.8\r\nConnection: close\r\n\r\n",
        path, host, USER_AGENT
    )
}

/// Toglie la codifica "chunked" (blocchi preceduti dalla lunghezza in esadecimale).
fn dechunk(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let Some(e) = b[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let line = core::str::from_utf8(&b[i..i + e]).unwrap_or("0");
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        i += e + 2;
        if size == 0 {
            break;
        }
        let end = (i + size).min(b.len());
        out.extend_from_slice(&b[i..end]);
        i = end + 2;
    }
    out
}

/// true quando la risposta e' arrivata tutta (Content-Length raggiunto o ultimo blocco "chunked").
fn response_complete(d: &[u8]) -> bool {
    let Some(he) = d.windows(4).position(|w| w == b"\r\n\r\n") else { return false };
    let Ok(head) = core::str::from_utf8(&d[..he]) else { return false };
    for l in head.lines() {
        if let Some((k, v)) = l.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                return v.trim().parse::<usize>().map(|n| d.len() >= he + 4 + n).unwrap_or(false);
            }
            if k.trim().eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked") {
                return d.ends_with(b"\r\n0\r\n\r\n") || (d.len() == he + 9 && d.ends_with(b"0\r\n\r\n"));
            }
        }
    }
    false
}

// ------------------------------------------------------------------ HTTPS (TLS 1.3)
//
// La crittografia e' fatta da `embedded-tls` (TLS 1.3, AES-128-GCM, chiavi P-256).
// I dati viaggiano sullo stesso socket TCP di smoltcp: TcpIo li legge e scrive
// facendo avanzare la rete mentre aspetta.
//
// Limite: il certificato del sito NON viene ancora verificato (la connessione e'
// cifrata, ma non c'e' la garanzia di parlare davvero con quel sito).

use embedded_tls::blocking::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext, TlsError, UnsecureProvider};

const TLS_READ_BUF: usize = 16 * 1024 + 512;
const TLS_WRITE_BUF: usize = 8 * 1024;
static mut TLS_RBUF: [u8; TLS_READ_BUF] = [0; TLS_READ_BUF];
static mut TLS_WBUF: [u8; TLS_WRITE_BUF] = [0; TLS_WRITE_BUF];
static mut PLAIN: [u8; 16 * 1024] = [0; 16 * 1024];

type Conn = TlsConnection<'static, TcpIo, Aes128GcmSha256>;
static mut CONN: Option<Conn> = None;

fn conn() -> &'static mut Option<Conn> {
    unsafe { &mut *core::ptr::addr_of_mut!(CONN) }
}

fn tls_drop() {
    *conn() = None;
}

/// Il socket TCP visto da embedded-tls (lettura e scrittura "bloccanti").
pub struct TcpIo {
    h: SocketHandle,
}

impl embedded_io::ErrorType for TcpIo {
    type Error = embedded_io::ErrorKind;
}

impl TcpIo {
    /// Fa avanzare la rete e lavora sul socket; si ferma dopo 15 secondi.
    fn wait<R>(&mut self, mut f: impl FnMut(&mut tcp::Socket) -> Option<Result<R, embedded_io::ErrorKind>>) -> Result<R, embedded_io::ErrorKind> {
        let deadline = now_ms() + 15_000;
        loop {
            let Some(n) = net() else { return Err(embedded_io::ErrorKind::NotConnected) };
            n.iface.poll(now(), &mut n.dev, &mut n.sockets);
            let sock = n.sockets.get_mut::<tcp::Socket>(self.h);
            if let Some(r) = f(sock) {
                n.iface.poll(now(), &mut n.dev, &mut n.sockets);
                return r;
            }
            if now_ms() > deadline {
                return Err(embedded_io::ErrorKind::TimedOut);
            }
            core::hint::spin_loop();
        }
    }
}

impl embedded_io::Read for TcpIo {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.wait(|s| {
            if s.can_recv() {
                Some(s.recv_slice(buf).map_err(|_| embedded_io::ErrorKind::Other))
            } else if !s.may_recv() {
                Some(Ok(0))
            } else {
                None
            }
        })
    }
}

impl embedded_io::Write for TcpIo {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.wait(|s| {
            if !s.may_send() {
                Some(Err(embedded_io::ErrorKind::BrokenPipe))
            } else if s.can_send() {
                match s.send_slice(buf) {
                    Ok(0) => None,
                    Ok(k) => Some(Ok(k)),
                    Err(_) => Some(Err(embedded_io::ErrorKind::Other)),
                }
            } else {
                None
            }
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        if let Some(n) = net() {
            n.iface.poll(now(), &mut n.dev, &mut n.sockets);
        }
        Ok(())
    }
}

/// Numeri casuali per le chiavi: RDRAND (se c'e') + contatore di tempo, mescolati con SHA-256.
pub struct Rng {
    state: [u8; 32],
    ctr: u64,
}

fn rdrand() -> Option<u64> {
    if crate::arch::cpuid(1).2 & (1 << 30) == 0 {
        return None;
    }
    for _ in 0..10 {
        let v: u64;
        let ok: u8;
        unsafe { core::arch::asm!("rdrand {0}", "setc {1}", out(reg) v, out(reg_byte) ok, options(nomem, nostack)) };
        if ok != 0 {
            return Some(v);
        }
    }
    None
}

impl Rng {
    pub fn new() -> Rng {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for i in 0..16u64 {
            h.update(rdtsc().to_le_bytes());
            h.update(rdrand().unwrap_or(i).to_le_bytes());
            // il tempo per fare un hash varia un po': altra entropia
            let t = rdtsc();
            let mut x = t;
            for _ in 0..(t & 0xFF) {
                x = x.rotate_left(7) ^ 0x9E37_79B9_7F4A_7C15;
            }
            h.update(x.to_le_bytes());
        }
        Rng { state: h.finalize().into(), ctr: 0 }
    }
}

impl rand_core::RngCore for Rng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        use sha2::{Digest, Sha256};
        for chunk in dest.chunks_mut(32) {
            self.ctr += 1;
            let mut h = Sha256::new();
            h.update(self.state);
            h.update(self.ctr.to_le_bytes());
            h.update(rdtsc().to_le_bytes());
            let out: [u8; 32] = h.finalize().into();
            chunk.copy_from_slice(&out[..chunk.len()]);
            // lo stato va avanti: chi vede l'uscita non ricava le chiavi passate
            let mut h2 = Sha256::new();
            h2.update(out);
            h2.update(self.state);
            self.state = h2.finalize().into();
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl rand_core::CryptoRng for Rng {}

fn tls_err(e: TlsError) -> String {
    match e {
        TlsError::HandshakeAborted(_, d) => format!("TLS: il sito ha rifiutato la connessione ({:?})", d),
        TlsError::Io(k) => format!("TLS: errore di rete ({:?})", k),
        e => format!("TLS: {:?}", e),
    }
}

/// Fa avanzare il download HTTPS in corso. Ritorna true se e' cambiato qualcosa.
fn tls_pump() -> bool {
    // 1. che cosa c'e' da fare? (si legge lo stato e si lascia subito la rete)
    let (host, path, stage, h) = {
        let Some(n) = net() else { return false };
        let Some(h) = n.tcp else { return false };
        match &n.task {
            Task::Get { tls: true, stage, host, path, .. } if *stage >= 3 => (host.clone(), path.clone(), *stage, h),
            _ => return false,
        }
    };

    if stage == 3 {
        // 2. stretta di mano TLS + richiesta HTTP
        let (rb, wb) = unsafe { (&mut *core::ptr::addr_of_mut!(TLS_RBUF), &mut *core::ptr::addr_of_mut!(TLS_WBUF)) };
        let mut c: Conn = TlsConnection::new(TcpIo { h }, rb, wb);
        let config = TlsConfig::new().with_server_name(&host).enable_rsa_signatures();
        let res = c.open(TlsContext::new(&config, UnsecureProvider::new::<Aes128GcmSha256>(Rng::new()))).and_then(|_| {
            let req = http_request(&host, &path);
            let mut sent = 0;
            while sent < req.len() {
                sent += c.write(&req.as_bytes()[sent..])?;
            }
            c.flush()
        });
        match res {
            Ok(()) => {
                *conn() = Some(c);
                if let Some(n) = net() {
                    if let Task::Get { stage, start, .. } = &mut n.task {
                        *stage = 4;
                        *start = now_ms();
                    }
                }
            }
            Err(e) => {
                crate::log!("TLS {}: {:?}", host, e);
                drop(c);
                tls_finish(Some(tls_err(e)), true);
            }
        }
        return true;
    }

    // 3. lettura dei dati cifrati (solo quando ne sono arrivati, per non bloccare lo schermo)
    let t0 = now_ms();
    let mut got = false;
    loop {
        let ready = {
            let Some(n) = net() else { return false };
            let s = n.sockets.get_mut::<tcp::Socket>(h);
            s.can_recv() || !s.may_recv()
        };
        if !ready {
            break;
        }
        let Some(c) = conn().as_mut() else { return false };
        let plain = unsafe { &mut *core::ptr::addr_of_mut!(PLAIN) };
        match c.read(plain) {
            Ok(0) => {
                tls_finish(None, false);
                return true;
            }
            Ok(k) => {
                got = true;
                let Some(n) = net() else { return false };
                let mut done = false;
                if let Task::Get { data, start, app, .. } = &mut n.task {
                    *start = now_ms();
                    let limit = if *app { APP_LIMIT } else { 512 * 1024 };
                    if data.len() < limit {
                        data.extend_from_slice(&plain[..k]);
                    } else {
                        done = true;
                    }
                    if *app {
                        n.fetch = Fetch::Busy(data.len());
                    }
                    if response_complete(data) {
                        done = true;
                    }
                }
                if done {
                    tls_finish(None, false);
                    return true;
                }
            }
            Err(TlsError::ConnectionClosed) | Err(TlsError::Io(_)) | Err(TlsError::HandshakeAborted(..)) => {
                // il sito ha chiuso: i dati ricevuti sono la risposta
                tls_finish(None, false);
                return true;
            }
            Err(e) => {
                tls_finish(Some(tls_err(e)), false);
                return true;
            }
        }
        if now_ms() > t0 + 40 {
            break;
        }
    }
    got
}

/// Chiude il download HTTPS consegnando la risposta (o l'errore).
fn tls_finish(err: Option<String>, handshake: bool) {
    tls_drop();
    let Some(n) = net() else { return };
    let task = core::mem::replace(&mut n.task, Task::Idle);
    n.close_tcp();
    let Task::Get { data, app, .. } = task else { return };
    match err {
        Some(e) if data.is_empty() || handshake => {
            if app {
                n.fetch = Fetch::Failed(e, handshake);
            } else {
                n.out.push(e);
            }
        }
        _ => {
            if app {
                n.finish_app(data, false);
            } else {
                n.print_http(&data);
            }
        }
    }
}
