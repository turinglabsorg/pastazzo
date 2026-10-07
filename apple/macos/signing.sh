#!/bin/sh
# Creates "Pastazzo Local Signing", a code signing certificate that exists
# only on this Mac. Signing every build of Pastazzo and pastazzo-sync with it
# gives them the same signature, so macOS keeps their keychain access and the
# Accessibility permission across updates.
#
# Run it on the Mac itself (Terminal), not over SSH: it needs the login
# keychain. macOS asks for your password once, to trust the certificate for
# code signing. Nothing leaves this Mac: the private key is created here and
# goes straight into the login keychain.
set -eu

NAME="${PASTAZZO_SIGN_IDENTITY:-Pastazzo Local Signing}"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-identity -v -p codesigning "$KEYCHAIN" | grep -q "\"$NAME\""; then
    echo "$NAME is already set up"
    exit 0
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cat > "$TMP/certificate.conf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF

/usr/bin/openssl req -x509 -newkey rsa:3072 -sha256 -days 3650 -nodes \
    -config "$TMP/certificate.conf" -keyout "$TMP/key.pem" -out "$TMP/certificate.pem" 2>/dev/null
# A throwaway passphrase for the temporary bundle the keychain imports.
PASSPHRASE=$(/usr/bin/openssl rand -hex 16)
/usr/bin/openssl pkcs12 -export -name "$NAME" -inkey "$TMP/key.pem" -in "$TMP/certificate.pem" \
    -passout "pass:$PASSPHRASE" -out "$TMP/identity.p12"
security import "$TMP/identity.p12" -k "$KEYCHAIN" -P "$PASSPHRASE" -T /usr/bin/codesign
echo "trusting $NAME for code signing: macOS asks for your password"
security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$TMP/certificate.pem"
security find-identity -v -p codesigning "$KEYCHAIN" | grep "\"$NAME\""
