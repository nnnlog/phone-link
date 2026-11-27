#!/bin/sh
# Builds the APT and DNF repositories from the packages in $1 into $2, signed with the GPG key whose
# fingerprint is $3 (already in the keyring).
#
#   $2/phonelink.gpg            the public key, binary (APT's Signed-By)
#   $2/phonelink.asc            the public key, armored (DNF's gpgkey)
#   $2/apt/pool/main/           the .deb files
#   $2/apt/dists/stable/        Packages for amd64, Release, InRelease, Release.gpg
#   $2/rpm/                     the signed .rpm files and repodata/ with repomd.xml.asc
#   $2/phonelink.sources        the APT source to drop in /etc/apt/sources.list.d/
#   $2/phonelink.repo           the DNF repository to drop in /etc/yum.repos.d/
#   $2/index.html               how to add the repositories, and the versions they hold
#
# Needs apt-ftparchive (apt-utils), dpkg-deb, createrepo_c, rpmsign (rpm) and gpg.
set -eu

packages=$1
site=$2
key=$3
url=${PAGES_URL:?PAGES_URL names the address the site is served at}

mkdir -p "$site/apt/pool/main" "$site/rpm"
gpg --batch --yes --export "$key" > "$site/phonelink.gpg"
gpg --batch --yes --armor --export "$key" > "$site/phonelink.asc"

cp "$packages"/*.deb "$site/apt/pool/main/"
cd "$site/apt"
mkdir -p dists/stable/main/binary-amd64
apt-ftparchive --arch amd64 packages pool > dists/stable/main/binary-amd64/Packages
gzip -9kf dists/stable/main/binary-amd64/Packages
apt-ftparchive \
    -o APT::FTPArchive::Release::Origin=PhoneLink \
    -o APT::FTPArchive::Release::Label=PhoneLink \
    -o APT::FTPArchive::Release::Suite=stable \
    -o APT::FTPArchive::Release::Codename=stable \
    -o APT::FTPArchive::Release::Architectures=amd64 \
    -o APT::FTPArchive::Release::Components=main \
    release dists/stable > dists/stable/Release
gpg --batch --yes --local-user "$key" --clearsign -o dists/stable/InRelease dists/stable/Release
gpg --batch --yes --local-user "$key" --armor --detach-sign -o dists/stable/Release.gpg dists/stable/Release
cd - > /dev/null

cp "$packages"/*.rpm "$site/rpm/"
rpmsign --define "_gpg_name $key" --define "__gpg $(command -v gpg)" --addsign "$site"/rpm/*.rpm
createrepo_c --quiet "$site/rpm"
gpg --batch --yes --local-user "$key" --armor --detach-sign -o "$site/rpm/repodata/repomd.xml.asc" "$site/rpm/repodata/repomd.xml"

cat > "$site/phonelink.sources" <<EOF
Types: deb
URIs: $url/apt
Suites: stable
Components: main
Signed-By: /usr/share/keyrings/phonelink.gpg
EOF

cat > "$site/phonelink.repo" <<EOF
[phonelink]
name=Phone Link
baseurl=$url/rpm
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=$url/phonelink.asc
EOF

versions=$(for deb in "$site"/apt/pool/main/*.deb; do dpkg-deb -f "$deb" Version; done | sort -Vru |
    sed 's|.*|      <li><code>&</code></li>|')

cat > "$site/index.html" <<EOF
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Phone Link for Linux</title>
  <style>
    :root { color-scheme: light dark; --fg: #1d1d1f; --muted: #6e6e73; --bg: #ffffff; --code: #f2f2f5; }
    @media (prefers-color-scheme: dark) {
      :root { --fg: #f5f5f7; --muted: #a1a1a6; --bg: #1c1c1e; --code: #2c2c2e; }
    }
    body { margin: 0; background: var(--bg); color: var(--fg);
           font: 16px/1.55 system-ui, -apple-system, "Segoe UI", sans-serif; }
    main { max-width: 760px; margin: 0 auto; padding: 40px 16px 64px; }
    h1 { font-size: 28px; margin: 0 0 4px; }
    h2 { font-size: 19px; margin: 32px 0 8px; }
    p { margin: 8px 0; }
    .lead { color: var(--muted); margin-bottom: 24px; }
    pre { background: var(--code); border-radius: 8px; padding: 12px 14px; overflow-x: auto;
          font-size: 14px; line-height: 1.5; }
    code { font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace; }
    li code, p code { background: var(--code); border-radius: 4px; padding: 1px 5px; }
  </style>
</head>
<body>
  <main>
    <h1>Phone Link for Linux</h1>
    <p class="lead">Package repositories for x86_64: Debian 12 or later, Ubuntu 24.04 or later and Fedora.
    Installed from here, Phone Link updates with the rest of the system.</p>

    <h2>Debian and Ubuntu</h2>
    <pre><code>sudo curl -fsSL $url/phonelink.gpg -o /usr/share/keyrings/phonelink.gpg
sudo curl -fsSL $url/phonelink.sources -o /etc/apt/sources.list.d/phonelink.sources
sudo apt update
sudo apt install phonelink</code></pre>

    <h2>Fedora</h2>
    <pre><code>sudo curl -fsSL $url/phonelink.repo -o /etc/yum.repos.d/phonelink.repo
sudo dnf install phonelink</code></pre>

    <h2>Versions</h2>
    <p>The repositories hold the latest release and the one before it, newest first:</p>
    <ul>
$versions
    </ul>
    <p>To go back to the earlier one: <code>sudo apt install --allow-downgrades phonelink=VERSION</code>
    or <code>sudo dnf downgrade phonelink</code>.</p>
  </main>
</body>
</html>
EOF
