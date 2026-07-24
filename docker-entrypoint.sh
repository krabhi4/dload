#!/bin/sh
# Run as PUID/PGID (LinuxServer.io / *arr convention) so the database and
# downloads are owned by the host user instead of root.
set -eu

PUID="${PUID:-1000}"
PGID="${PGID:-1000}"

echo "[dload] starting as PUID=${PUID} PGID=${PGID}"

mkdir -p /data /downloads

# chown only mis-owned entries so restarts skip a full re-chown of /downloads.
fix_owner() {
    find "$1" \( ! -uid "$PUID" -o ! -gid "$PGID" \) \
        -exec chown "${PUID}:${PGID}" {} + 2>/dev/null || true
}
fix_owner /data
fix_owner /downloads

# librqbit writes DHT state under $HOME (via the `directories` crate). gosu resets
# $HOME to the target user's home dir (/ for a PUID with no /etc/passwd entry), so
# exporting HOME *before* gosu does not survive the exec. Set it on gosu's command
# via `env` instead, pointing librqbit at the writable /data volume.
exec gosu "${PUID}:${PGID}" env HOME=/data "$@"
