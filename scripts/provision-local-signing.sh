#!/usr/bin/env bash
# Create a local self-signed code-signing identity ("Zeron Local Signing") in
# the login keychain, so every Zeron build carries the same designated
# requirement. macOS privacy grants (Accessibility, Screen Recording) are tied
# to that requirement: with ad-hoc signing it is the binary's cdhash and every
# rebuild silently loses them; with a stable certificate they survive updates.
#
# The certificate is not trusted system-wide and is not a Developer ID: it
# cannot notarize, and Gatekeeper still treats the app as unidentified. It
# only gives TCC a stable identity on this Mac.
#
# Usage: scripts/provision-local-signing.sh        (idempotent)
# Undo:  security delete-identity -c "Zeron Local Signing" ~/Library/Keychains/login.keychain-db

set -euo pipefail

NAME="${ZERON_LOCAL_SIGNING_NAME:-Zeron Local Signing}"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-identity -p codesigning "$KEYCHAIN" 2>/dev/null | grep -q "\"$NAME\""; then
  echo "identity already present: $NAME"
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
PASS="$(/usr/bin/openssl rand -hex 16)"

cat >"$WORK/cert.cnf" <<EOF
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF

# 10-year RSA identity; LibreSSL's PKCS#12 defaults are what `security` imports.
/usr/bin/openssl req -new -x509 -newkey rsa:2048 -nodes -days 3650 -sha256 \
  -config "$WORK/cert.cnf" -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null
/usr/bin/openssl pkcs12 -export -name "$NAME" -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
  -out "$WORK/identity.p12" -passout "pass:$PASS"

# -T: let codesign use the private key without a keychain prompt per build.
security import "$WORK/identity.p12" -k "$KEYCHAIN" -P "$PASS" -f pkcs12 \
  -T /usr/bin/codesign -T /usr/bin/security

security find-identity -p codesigning "$KEYCHAIN" | grep "\"$NAME\"" || {
  echo "error: imported identity is not usable for code signing" >&2
  exit 1
}
echo "created: $NAME"
