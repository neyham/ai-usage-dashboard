#!/usr/bin/env sh
# Install AI Usage Dashboard on Linux from the latest (or pinned) GitHub Release.
# Prefer .deb on Debian/Ubuntu; fall back to AppImage for other distributions.
#
# Usage:
#   curl -fsSL https://github.com/neyham/ai-usage-dashboard/releases/latest/download/install-linux.sh | sh
#   VERSION=0.5.0 sh install-linux.sh
#   REPO=neyham/ai-usage-dashboard sh install-linux.sh
set -eu

REPO="${REPO:-neyham/ai-usage-dashboard}"
VERSION="${VERSION:-}"
INSTALL_PREFIX="${INSTALL_PREFIX:-$HOME/.local}"
APPIMAGE_DIR="${APPIMAGE_DIR:-$INSTALL_PREFIX/bin}"
TMPDIR="${TMPDIR:-/tmp}"
WORKDIR=""

cleanup() {
  if [ -n "${WORKDIR}" ] && [ -d "${WORKDIR}" ]; then
    rm -rf "${WORKDIR}"
  fi
}
trap cleanup EXIT INT TERM

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

need_cmd uname
need_cmd mktemp
need_cmd curl
need_cmd sha256sum

arch="$(uname -m)"
case "${arch}" in
  x86_64|amd64) arch_deb="amd64"; arch_tag="amd64" ;;
  aarch64|arm64) arch_deb="arm64"; arch_tag="aarch64" ;;
  *) die "unsupported architecture: ${arch}" ;;
esac

if [ -z "${VERSION}" ]; then
  # Resolve latest tag via the GitHub API redirect for /releases/latest.
  latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest")"
  VERSION="${latest_url##*/}"
  VERSION="${VERSION#v}"
  [ -n "${VERSION}" ] || die "could not resolve latest release version"
fi

VERSION="${VERSION#v}"
tag="v${VERSION}"
base="https://github.com/${REPO}/releases/download/${tag}"
sums_name="AI-Usage-Dashboard_${VERSION}_SHA256SUMS.txt"

WORKDIR="$(mktemp -d "${TMPDIR}/ai-usage-dashboard-install.XXXXXX")"
cd "${WORKDIR}"

printf 'Downloading checksums for %s...\n' "${tag}"
curl -fsSL -o "${sums_name}" "${base}/${sums_name}" \
  || die "failed to download ${sums_name} (is ${tag} published with Linux assets?)"

verify() {
  # $1 = file name present in the current directory and in SUMS
  file="$1"
  line="$(grep -E "[[:space:]]${file}\$" "${sums_name}" || true)"
  [ -n "${line}" ] || die "checksum entry missing for ${file}"
  expected="$(printf '%s\n' "${line}" | awk '{print $1}')"
  actual="$(sha256sum "${file}" | awk '{print $1}')"
  [ "${expected}" = "${actual}" ] || die "SHA-256 mismatch for ${file}"
  printf 'Verified %s\n' "${file}"
}

install_deb() {
  deb="AI-Usage-Dashboard_${VERSION}_${arch_deb}.deb"
  printf 'Downloading %s...\n' "${deb}"
  curl -fsSL -o "${deb}" "${base}/${deb}" || return 1
  verify "${deb}"
  if [ "$(id -u)" -eq 0 ]; then
    dpkg -i "./${deb}" || apt-get install -y -f "./${deb}"
  else
    need_cmd sudo
    sudo dpkg -i "./${deb}" || sudo apt-get install -y -f "./${deb}"
  fi
  printf 'Installed AI Usage Dashboard %s via .deb\n' "${VERSION}"
  printf 'Try: ai-usage-dashboard --judge-demo\n'
}

install_appimage() {
  # Use only the native architecture; never fall back to a foreign binary.
  appimage="AI-Usage-Dashboard_${VERSION}_${arch_tag}.AppImage"
  curl -fsSL -o "${appimage}" "${base}/${appimage}" || return 1
  verify "${appimage}"
  mkdir -p "${APPIMAGE_DIR}"
  dest="${APPIMAGE_DIR}/ai-usage-dashboard.AppImage"
  install -m 755 "${appimage}" "${dest}"
  printf 'Installed AppImage to %s\n' "${dest}"
  printf 'Run: %s --judge-demo\n' "${dest}"
}

if command -v dpkg >/dev/null 2>&1 && command -v apt-get >/dev/null 2>&1; then
  if install_deb; then
    exit 0
  fi
  printf 'deb install unavailable; trying AppImage...\n' >&2
fi

if install_appimage; then
  exit 0
fi

die "no installable Linux asset found for ${tag} (${arch}). Check the GitHub Release assets."
