#!/usr/bin/env bash
# Prova di avvio: MIN-OS deve arrivare all'installer senza errori, da BIOS e da UEFI.
# Uso: tools/smoke-test.sh target/min-os.img
set -u
IMG="$1"
fail=0
boot() {
  local name="$1"; shift
  local log="smoke-$name.log"
  cp "$IMG" smoke.img
  timeout 120 qemu-system-x86_64 -drive format=raw,file=smoke.img -m 512M -display none \
    -serial "file:$log" -no-reboot -nic user,model=e1000 "$@" &
  local pid=$!
  for _ in $(seq 1 110); do
    sleep 1
    if grep -qa "PCI:" "$log" 2>/dev/null && grep -qa "rete\|DHCP" "$log" 2>/dev/null; then break; fi
  done
  sleep 5
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  echo "----- $name -----"; cat "$log"
  if grep -qa "kernel avviato" "$log" && ! grep -qa "ERRORE" "$log"; then
    echo "OK: avvio $name riuscito"
  else
    echo "ERRORE: avvio $name non riuscito"; fail=1
  fi
}
boot bios
cp /usr/share/OVMF/OVMF_VARS_4M.fd vars.fd
boot uefi -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd -drive if=pflash,format=raw,file=vars.fd
rm -f smoke.img vars.fd
exit $fail
