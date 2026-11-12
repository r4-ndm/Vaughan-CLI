#!/usr/bin/env bash
# Install script for Vaughan isolated signer daemon (systemd service).
#
# Usage (run once as root):
#   sudo bash scripts/install-signerd.sh [--force] [--owner-uid <uid>]
#
# Requirements:
#   - Creates dedicated unprivileged system account `vaughan-signer` (no login shell, no home).
#   - Creates system group `vaughan` and adds owner user to it.
#   - Creates /var/lib/vaughan (0750) and /var/lib/vaughan/sentient (0700).
#   - Creates /run/vaughan (0755) owned by vaughan-signer:vaughan.
#   - Creates /etc/vaughan (0750) and /etc/vaughan/sentient.env (0640).
#   - Installs systemd service unit /etc/systemd/system/vaughan-signerd.service.
#   - Idempotent: safe to run multiple times; refuses to overwrite /etc/vaughan/sentient.env without --force.

set -euo pipefail

FORCE=0
OWNER_UID_OVERRIDE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --force|-f)
            FORCE=1
            shift
            ;;
        --owner-uid)
            OWNER_UID_OVERRIDE="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: sudo bash scripts/install-signerd.sh [--force] [--owner-uid <uid>]"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
    echo "error: scripts/install-signerd.sh must be run as root (e.g. sudo bash scripts/install-signerd.sh)" >&2
    exit 1
fi

# Determine owner UID and user
OWNER_UID="${OWNER_UID_OVERRIDE:-${SUDO_UID:-$(id -u)}}"
OWNER_USER="${SUDO_USER:-}"

if [[ -z "$OWNER_USER" && -n "$OWNER_UID" ]]; then
    OWNER_USER="$(id -nu "$OWNER_UID" 2>/dev/null || echo "")"
fi

if [[ -z "$OWNER_UID" || "$OWNER_UID" -eq 0 ]]; then
    if [[ -z "$OWNER_UID_OVERRIDE" ]]; then
        echo "warning: running as root without SUDO_UID; pass --owner-uid <uid> to specify the agent/human owner UID" >&2
    fi
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

SIGNER_USER="vaughan-signer"
SIGNER_GROUP="vaughan-signer"
VAUGHAN_GROUP="vaughan"

echo "==> Setting up system accounts and groups"

# Create vaughan-signer group if not present
if ! getent group "$SIGNER_GROUP" >/dev/null 2>&1; then
    groupadd --system "$SIGNER_GROUP"
fi

# Create vaughan group (for socket access) if not present
if ! getent group "$VAUGHAN_GROUP" >/dev/null 2>&1; then
    groupadd --system "$VAUGHAN_GROUP"
fi

# Create vaughan-signer system user if not present
if ! id -u "$SIGNER_USER" >/dev/null 2>&1; then
    useradd --system \
        --no-create-home \
        --shell /usr/sbin/nologin \
        --gid "$SIGNER_GROUP" \
        "$SIGNER_USER"
fi

# Add vaughan-signer to vaughan group
usermod -a -G "$VAUGHAN_GROUP" "$SIGNER_USER" || true

# Add owner user to vaughan group (so agent/TUI can access socket)
if [[ -n "$OWNER_USER" && "$OWNER_USER" != "root" ]]; then
    usermod -a -G "$VAUGHAN_GROUP" "$OWNER_USER" || true
    echo "Added user $OWNER_USER to group $VAUGHAN_GROUP"
fi

echo "==> Creating directories with correct permissions"

# 1. /var/lib/vaughan and sentient store
mkdir -p /var/lib/vaughan/sentient
chown -R "$SIGNER_USER:$SIGNER_GROUP" /var/lib/vaughan
chmod 0750 /var/lib/vaughan
chmod 0700 /var/lib/vaughan/sentient

# 2. /run/vaughan runtime directory
mkdir -p /run/vaughan
chown "$SIGNER_USER:$VAUGHAN_GROUP" /run/vaughan
chmod 0755 /run/vaughan

# 3. /etc/vaughan configuration directory
mkdir -p /etc/vaughan
chown root:"$SIGNER_GROUP" /etc/vaughan
chmod 0750 /etc/vaughan

ENV_FILE="/etc/vaughan/sentient.env"
if [[ -f "$ENV_FILE" && "$FORCE" -eq 0 ]]; then
    echo "Notice: $ENV_FILE already exists, preserving (use --force to overwrite)."
else
    echo "==> Configuring $ENV_FILE"
    # If the user previously had serve.env in ~/.config/vaughan/serve.env, attempt to migrate password
    IMPORTED_PASS=0
    if [[ -n "$OWNER_USER" ]]; then
        OWNER_HOME="$(getent passwd "$OWNER_USER" | cut -d: -f6)"
        OLD_SERVE_ENV="$OWNER_HOME/.config/vaughan/serve.env"
        if [[ -f "$OLD_SERVE_ENV" ]]; then
            echo "Importing VAUGHAN_WALLET_PASSWORD from $OLD_SERVE_ENV"
            grep -E '^VAUGHAN_WALLET_PASSWORD=' "$OLD_SERVE_ENV" > "$ENV_FILE" || true
            if [[ -s "$ENV_FILE" ]]; then
                IMPORTED_PASS=1
            fi
        fi
    fi

    if [[ "$IMPORTED_PASS" -eq 0 ]]; then
        cat > "$ENV_FILE" <<'EOF'
# Vaughan isolated signer environment
# Set the vault password here:
VAUGHAN_WALLET_PASSWORD=
EOF
    fi

    chown root:"$SIGNER_USER" "$ENV_FILE"
    chmod 0640 "$ENV_FILE"
fi

echo "==> Installing systemd unit"

SERVICE_SRC="$REPO_ROOT/scripts/systemd/vaughan-signerd.service"
SERVICE_DST="/etc/systemd/system/vaughan-signerd.service"

if [[ ! -f "$SERVICE_SRC" ]]; then
    echo "error: systemd service template not found at $SERVICE_SRC" >&2
    exit 1
fi

# Substitute @OWNER_UID@ with the detected/specified owner UID
sed "s/@OWNER_UID@/$OWNER_UID/g" "$SERVICE_SRC" > "$SERVICE_DST"
chmod 0644 "$SERVICE_DST"

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
    echo "Systemd unit installed: $SERVICE_DST (run: systemctl enable --now vaughan-signerd)"
else
    echo "Installed unit to $SERVICE_DST (systemctl not detected)"
fi

echo
echo "==> Installation complete."
echo "Next steps:"
echo "  1. If not imported automatically, set VAUGHAN_WALLET_PASSWORD in $ENV_FILE"
echo "  2. Install the binary where the unit expects it:"
echo "       sudo install -m 0755 <path-to-vaughan-0.2.2+> /usr/local/bin/vaughan"
echo "  3. Migrate the profile into the store. migrate refuses unless the target dir is owned by the"
echo "     invoking uid, and $SIGNER_USER cannot read your home, so let root do the move:"
echo "       sudo chown root:root /var/lib/vaughan/sentient"
echo "       sudo /usr/local/bin/vaughan signerd migrate --profile sentient --store-root /var/lib/vaughan/sentient"
echo "       sudo chown -R $SIGNER_USER:$SIGNER_GROUP /var/lib/vaughan/sentient"
echo "  4. Log out and back in (or run 'newgrp $VAUGHAN_GROUP') so the $VAUGHAN_GROUP group applies."
echo "  5. Start the isolated signer daemon:"
echo "       systemctl enable --now vaughan-signerd"
echo "       systemctl status vaughan-signerd --no-pager"
