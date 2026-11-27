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
#
# Needs apt-ftparchive (apt-utils), createrepo_c, rpmsign (rpm) and gpg.
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
