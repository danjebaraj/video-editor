#!/bin/zsh
# Fills in Signing.local.xcconfig from the certificates in this Mac's keychain:
#   CODE_SIGN_IDENTITY / DEVELOPMENT_TEAM   ← "Apple Development: …"   (everyday builds)
#   VS_DISTRIBUTION_IDENTITY                ← "Developer ID Application: …" (./build.sh distribute)
#   VS_NOTARY_PROFILE                       ← kept, or "videostudio-notary"
# Certificates are referenced by their SHA-1 hash, so two certificates with the same name can't
# be confused. Run again after adding or renewing a certificate. Usage: ./build.sh signing
setopt pipefail
cd "${0:A:h}/.."
CONF=Signing.local.xcconfig

# "<hash> <name>" for each valid code-signing identity of the given kind
identities() {
  security find-identity -v -p codesigning 2>/dev/null \
    | sed -nE "s/^ *[0-9]+\) ([0-9A-F]{40}) \"($1: .*)\"$/\1 \2/p"
}
# Team ID = the certificate's OU
team_of() {
  security find-certificate -a -Z -p 2>/dev/null | awk -v h="$1" '
    /^SHA-1 hash:/ { keep = ($3 == h) }
    keep && /BEGIN CERTIFICATE/ { pem = "" ; inpem = 1 }
    keep && inpem { pem = pem $0 "\n" }
    keep && /END CERTIFICATE/ { print pem; exit }' \
    | openssl x509 -noout -subject -nameopt multiline 2>/dev/null \
    | sed -nE 's/^ *organizationalUnitName *= *([A-Z0-9]{10}).*/\1/p' | head -1
}
pick() { # $1 kind → "<hash> <name>" of the newest-listed one; warns when there are several
  local all; all=$(identities "$1")
  local n; n=$(print -r -- "$all" | grep -c . )
  if (( n > 1 )); then
    print -u2 "note: $n \"$1\" certificates found; using the last one listed:"
    print -r -u2 -- "$all" | sed 's/^/  /'
  fi
  print -r -- "$all" | tail -1
}
setting() { [[ -f $CONF ]] && sed -nE "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*(.*[^[:space:]])[[:space:]]*$/\1/p" $CONF | tail -1 }

dev=$(pick "Apple Development")
dist=$(pick "Developer ID Application")
if [[ -z $dev && -z $dist ]]; then
  cat >&2 <<'EOF'
No Apple signing certificates in this Mac's keychain.
  1. Xcode → Settings → Accounts → + → Apple ID, and sign in with your developer account.
  2. Select your team → Manage Certificates… → + → "Apple Development"
     (and "Developer ID Application" to share the app; only the Account Holder can create it,
      or import a .p12 someone exported for you by double-clicking it).
  3. Run ./build.sh signing again.
EOF
  exit 1
fi

dev_hash=${dev%% *}; dev_name=${dev#* }
dist_hash=${dist%% *}; dist_name=${dist#* }
team=""
[[ -n $dist ]] && team=$(team_of $dist_hash)
[[ -z $team && -n $dev ]] && team=$(team_of $dev_hash)
profile=$(setting VS_NOTARY_PROFILE); profile=${profile:-videostudio-notary}

# everyday builds: Apple Development, else Developer ID
if [[ -n $dev ]]; then every_hash=$dev_hash every_name=$dev_name; else every_hash=$dist_hash every_name=$dist_name; fi

[[ -f $CONF ]] && cp $CONF $CONF.bak
{
  print "// Written by ./build.sh signing on $(date '+%Y-%m-%d'). See Signing.local.xcconfig.example."
  print "// Everyday builds: $every_name"
  print "CODE_SIGN_IDENTITY = $every_hash"
  print "DEVELOPMENT_TEAM = $team"
  if [[ -n $dist ]]; then
    print "// ./build.sh distribute: $dist_name"
    print "VS_DISTRIBUTION_IDENTITY = $dist_hash"
  else
    print "// No Developer ID Application certificate yet; ./build.sh distribute needs one."
    print "VS_DISTRIBUTION_IDENTITY ="
  fi
  print "VS_NOTARY_PROFILE = $profile"
} > $CONF

print "✔ wrote $PWD/$CONF"
print "  everyday builds : $every_name"
print "  team            : ${team:-?}"
print "  distribute      : ${dist_name:-(no Developer ID Application certificate)}"
if [[ -n $dist ]]; then
  if xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1; then
    print "  notarization    : keychain profile \"$profile\" works"
  else
    print "  notarization    : not set up. Create the profile once (asks for an app-specific password):"
    print "    xcrun notarytool store-credentials $profile --apple-id <your Apple ID> --team-id ${team:-TEAMID}"
  fi
fi
