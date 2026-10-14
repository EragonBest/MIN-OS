# MIN-OS 0.5

Un piccolo sistema operativo scritto **da zero in Rust**: bootloader BIOS e UEFI, kernel, grafica
Liquid Glass, installer in 5 lingue, desktop in stile Windows 11, file system, app, USB e rete.
Nessun bootloader esterno, nessuna libreria grafica, nessuna libreria UEFI.

## Cosa c'è dentro

| Parte | File | Cosa fa |
|---|---|---|
| Bootloader BIOS | `kernel/src/boot.S` | Settore di avvio + stage 2 in assembly: carica il kernel, legge la memoria (E820), imposta la grafica VBE, attiva il paging ed entra in modalità 64 bit |
| Bootloader UEFI | `uefi/src/main.rs` | Applicazione UEFI scritta a mano: grafica con GOP, tabelle ACPI, page table, uscita dal firmware, avvio del kernel |
| Kernel | `kernel/src/main.rs`, `arch.rs`, `mem.rs` | Heap, timer, orologio, CPUID, eccezioni con schermata di errore |
| ACPI | `acpi.rs` | Spegnimento reale (stato S5) e conteggio dei core (MADT) |
| Dischi | `ata.rs`, `ahci.rs`, `nvme.rs`, `block.rs` | Driver IDE, SATA (AHCI) e NVMe: MIN-OS ritrova il proprio disco ovunque sia |
| File system | `fat.rs`, `storage.rs` | FAT16/FAT32 con nomi lunghi: cartelle, file, rinomina, sposta, cestino; impostazioni salvate su disco |
| USB | `usb.rs` | Controller xHCI: tastiera, mouse e chiavette/dischi USB |
| App | `apps.rs`, `web.rs`, `clip.rs` | Esplora file, Blocco note, Calcolatrice, Cestino, Internet, appunti (copia/incolla) |
| Rete | `pci.rs`, `e1000.rs`, `net.rs` | Bus PCI, driver Intel PRO/1000, TCP/IP con smoltcp: DHCP, DNS, ping, HTTP/1.1, **HTTPS (TLS 1.3)**, download |
| Grafica | `gfx.rs`, `font.rs` | Forme con antialiasing, ombre, materiale Liquid Glass, sfondo generato matematicamente |
| Input | `input.rs`, `keymap.rs` | Tastiera e mouse PS/2 o USB (rotellina, tasto destro, tasto Windows), layout IT / US / UK / DE / FR / ES |
| Installer | `installer.rs`, `i18n.rs` | Lingua, tastiera, account, aspetto, ottimizzazione, riepilogo, salvataggio |
| Desktop | `desktop.rs`, `ui.rs`, `i18n_desk.rs` | Icone, barra delle applicazioni, Start con ricerca, menu del tasto destro, finestre, Terminale, Impostazioni |
| Build | `xtask/src/main.rs` | Compila tutto, converte il loader in formato PE/EFI e crea l'immagine ibrida |

## Provarlo subito (senza compilare)

1. Installa QEMU per Windows: https://www.qemu.org/download/#windows
2. Apri il terminale nella cartella `MIN-OS`.

**Avvio classico (BIOS):**
```
"C:\Program Files\qemu\qemu-system-x86_64.exe" -drive format=raw,file=immagine\min-os.img -m 512M -smp 4 -rtc base=localtime -nic user,model=e1000 -accel whpx,kernel-irqchip=off -accel tcg
```

**Avvio moderno (UEFI):** aggiungi
```
-drive if=pflash,format=raw,readonly=on,file="C:\Program Files\qemu\share\edk2-x86_64-code.fd"
```

Clicca nella finestra di QEMU per usare il mouse, **Ctrl+Alt+G** per liberarlo.
Per l'accelerazione WHPX attiva "Piattaforma Windows Hypervisor" nelle funzionalità di Windows.

La prima volta parte l'installer; dalle volte successive MIN-OS va dritto al desktop con le tue impostazioni
(sono salvate dentro `min-os.img`). Per ricominciare: Impostazioni > Sistema > Ripristina MIN-OS.

## Aggiornamenti automatici da GitHub

A ogni `git push` su `main`, GitHub Actions (`.github/workflows/release.yml`):
1. compila MIN-OS con un numero di build nuovo (0.5.1, 0.5.2, ...);
2. lo avvia in QEMU (BIOS e UEFI) per controllare che parta (`tools/smoke-test.sh`);
3. firma l'aggiornamento e pubblica una **Release** con `update.bin`, `latest.txt` e `min-os.img`.

MIN-OS controlla da solo all'avvio e mostra una notifica; in **Impostazioni > Aggiornamenti** c'è
"Scarica e installa", poi "Riavvia ora". Il messaggio del commit appare come "Novità".
Prima di installare, MIN-OS salva la versione attuale: "Ripristina versione precedente" la rimette.

**Sicurezza**: gli aggiornamenti sono firmati (ECDSA P-256). La chiave pubblica è dentro il kernel
(`kernel/src/update_key.bin`); MIN-OS rifiuta qualsiasi pacchetto non firmato con la tua chiave privata.

**Configurazione (una volta sola)**
1. La chiave privata è `chiave-aggiornamenti.pem` (è in `.gitignore`: **non va mai su GitHub** e non va persa,
   altrimenti i MIN-OS già installati non accetteranno più aggiornamenti).
2. Su GitHub: repository > *Settings* > *Secrets and variables* > *Actions* > *New repository secret*:
   nome `MINOS_SIGN_KEY`, valore = tutto il contenuto di `chiave-aggiornamenti.pem`.
3. Il repository deve essere **pubblico** (MIN-OS scarica le release senza password).

Per scaricare MIN-OS già pronto: pagina **Releases** del repository, file `min-os.img`.

## Compilare

Serve solo Rust (https://rustup.rs). Il file `rust-toolchain.toml` installa da solo il target necessario.

```
cargo run -- run            # compila e avvia in QEMU (BIOS)
cargo run -- run --uefi     # compila e avvia in QEMU (UEFI)
cargo run -- build          # crea solo target\min-os.img
cargo run -- vdi            # crea anche un disco per VirtualBox
cargo run -- release        # come GitHub: build firmata in target\dist (serve la chiave)
```

Opzioni: `--debug`, `--no-accel`, `--no-net`.

## Su un PC vero

Scrivi `immagine\min-os.img` su una chiavetta USB con **Rufus** (modalità "DD") o balenaEtcher.
La stessa chiavetta si avvia sia in modalità **UEFI** sia **Legacy/CSM**. Per UEFI disattiva il **Secure Boot**
(MIN-OS non è firmato). Usa un computer di prova.

## Usare il desktop

- **Doppio clic** su un'icona per aprirla, **tasto destro** sul desktop per il menu
  (Visualizza, Ordina per, Aggiorna, **Nuovo > Cartella / Documento di testo**, Apri nel Terminale, Personalizza).
- **Selezione come in Windows**: trascina sul desktop per selezionare più icone con il rettangolo,
  **Ctrl+clic** per aggiungere/togliere, **Ctrl+A** tutto, **Canc** sposta nel Cestino, **F2** rinomina,
  **Invio** apre, **F5** aggiorna. Il nuovo nome si scrive subito (il vecchio è già selezionato).
- I file e le cartelle creati sul desktop stanno davvero sul disco, nella cartella `Desktop`.
- **Tasto Windows** o il logo sulla barra: menu Start. Scrivi per cercare, Invio apre il primo risultato.
- Finestre: trascina dalla barra del titolo, doppio clic sul titolo per ingrandire.

## Le app

- **Esplora file**: Documenti, Desktop, Download; nuova cartella, nuovo file, rinomina (F2), elimina (Canc),
  tasto destro per il menu, Backspace per tornare su. I file `.html` si aprono in Internet, gli altri nel Blocco note.
- **Internet**: siti `https://` e `http://`. Scrivi un indirizzo (`google.com`, `it.m.wikipedia.org`) o delle
  parole da cercare (ricerca con DuckDuckGo). Funzionano i link e le **caselle di ricerca dei siti** (Google,
  Wikipedia...): clic nella casella, scrivi, Invio. Indietro (anche Backspace), Ricarica (F5), Home,
  rotellina/PagSu/PagGiù per scorrere, Ctrl+L per la barra degli indirizzi.
  I file che non sono pagine (zip, immagini, pdf...) vengono **scaricati nella cartella Download**; il pulsante
  **Scarica** (o Ctrl+S) salva la pagina aperta. Poi "Apri cartella".
  Se un sito non accetta la connessione sicura di MIN-OS, si apre da solo in versione leggera tramite FrogFind.
- **Blocco note** (Ctrl+S salva), **Calcolatrice**, **Cestino** (Svuota / Ripristina).

## Il Terminale

- **Freccia su / giù**: richiama i comandi già inviati (come in Windows). Freccia sinistra/destra, Home, Fine
  per muoversi nella riga, **Tab** completa comandi e nomi di file.
- **Seleziona con il mouse** e copia con **Ctrl+C** o tasto destro; **Ctrl+V** o tasto destro incolla.
  Ctrl+A seleziona tutto, Ctrl+L pulisce, rotellina o PagSu/PagGiù per scorrere.

| Comando | Cosa fa |
|---|---|
| `ls`, `cd`, `pwd` | elenca e cambia cartella (es. `cd Documents`, `cd ..`) |
| `cat <file>`, `edit <file>` | mostra un file / lo apre nel Blocco note |
| `mkdir`, `touch`, `rm` | crea cartella / crea file vuoto / elimina definitivamente |
| `echo testo > file` | scrive un file |
| `internet [indirizzo]` | apre l'app Internet |
| `ip` | scheda di rete, MAC, indirizzo IP, gateway, DNS |
| `ping <host>` | 4 ping (es. `ping 10.0.2.2`) |
| `dns <nome> [server]` | risolve un nome (es. `dns example.com`) |
| `get https://...` | scarica una pagina web (anche HTTPS) e la mostra come testo |
| `info`, `time`, `whoami`, `uname` | informazioni sul sistema |
| `theme dark/light`, `accent 1-6`, `lang it/en/es/fr/de` | aspetto e lingua |
| `settings`, `clear`, `reboot`, `shutdown` | varie |

## Limiti attuali (prossimi passi)

- **HTTPS**: cifrato con TLS 1.3 (libreria `embedded-tls`: chiavi P-256, AES-128-GCM), ma il **certificato del
  sito non viene ancora verificato**: la connessione è cifrata, però non c'è la garanzia di parlare davvero con
  quel sito. Non usarlo per password o dati importanti. I siti che accettano solo TLS 1.2 si aprono in versione
  leggera tramite FrogFind (solo pagine, niente download).
- **Google**: la ricerca di Google oggi richiede JavaScript, che MIN-OS non ha: la pagina potrebbe chiedere di
  attivarlo. La ricerca dalla barra degli indirizzi usa DuckDuckGo, che funziona senza JavaScript.
- **Rete**: solo schede Intel PRO/1000 (quelle emulate da QEMU, VirtualBox e VMware), niente Wi-Fi.
- **Pagine web**: niente immagini, CSS e JavaScript; moduli solo con caselle di testo; download fino a 16 MB.
- **Multi-core**: i core vengono rilevati dalle tabelle ACPI, ma MIN-OS ne usa ancora uno solo.
- Nessun multitasking e nessuna applicazione esterna (tutto è dentro il kernel).

## Ringraziamenti e licenze

- L'architettura di ACPI, rete e supporto multi-piattaforma è ispirata a **Redox OS** (MIT), riscritta da zero.
- Rete: **smoltcp** (0BSD). HTTPS: **embedded-tls** (Apache-2.0) con le librerie crittografiche RustCrypto (MIT/Apache-2.0). Caratteri: **Noto Sans Mono** via `noto-sans-mono-bitmap` (SIL OFL).
  Allocatore: `linked_list_allocator` (MIT/Apache-2.0). Partizione EFI: `fatfs` (MIT, solo in fase di build).
