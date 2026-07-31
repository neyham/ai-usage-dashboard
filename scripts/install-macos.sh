#!/usr/bin/env sh
# Install AI Usage Dashboard on macOS from the latest (or pinned) GitHub Release.
# Downloads a .dmg, verifies SHA-256, and copies the app into Applications.
#
# Usage:
#   curl -fsSL https://github.com/neyham/ai-usage-dashboard/releases/latest/download/install-macos.sh | sh
#   VERSION=0.5.0 sh install-macos.sh
#   REPO=neyham/ai-usage-dashboard APPLICATIONS_DIR="$HOME/Applications" sh install-macos.sh
set -eu

REPO="${REPO:-neyham/ai-usage-dashboard}"
VERSION="${VERSION:-}"
APPLICATIONS_DIR="${APPLICATIONS_DIR:-/Applications}"
TMPDIR="${TMPDIR:-/tmp}"
WORKDIR=""
MOUNTPOINT=""

cleanup() {
  if [ -n "${MOUNTPOINT}" ] && [ -d "${MOUNTPOINT}" ]; then
    hdiutil detach "${MOUNTPOINT}" >/dev/null 2>&1 || true
  fi
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
need_cmd shasum
need_cmd hdiutil
need_cmd ditto

[ "$(uname -s)" = "Darwin" ] || die "this installer is for macOS only"

arch="$(uname -m)"
case "${arch}" in
  arm64) arch_tag="aarch64" ;;
  x86_64) arch_tag="x64" ;;
  *) die "unsupported architecture: ${arch}" ;;
esac

if [ -z "${VERSION}" ]; then
  latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest")"
  VERSION="${latest_url##*/}"
  VERSION="${VERSION#v}"
  [ -n "${VERSION}" ] || die "could not resolve latest release version"
fi

VERSION="${VERSION#v}"
tag="v${VERSION}"
base="https://github.com/${REPO}/releases/download/${tag}"
sums_name="AI-Usage-Dashboard_${VERSION}_SHA256SUMS.txt"
dmg="AI-Usage-Dashboard_${VERSION}_${arch_tag}.dmg"

WORKDIR="$(mktemp -d "${TMPDIR}/ai-usage-dashboard-install.XXXXXX")"
cd "${WORKDIR}"

printf 'Downloading checksums for %s...\n' "${tag}"
curl -fsSL -o "${sums_name}" "${base}/${sums_name}" \
  || die "failed to download ${sums_name} (is ${tag} published with macOS assets?)"

printf 'Downloading %s...\n' "${dmg}"
curl -fsSL -o "${dmg}" "${base}/${dmg}" \
  || die "failed to download ${dmg}"

line="$(grep -E "[[:space:]]${dmg}\$" "${sums_name}" || true)"
[ -n "${line}" ] || die "checksum entry missing for ${dmg}"
expected="$(printf '%s\n' "${line}" | awk '{print $1}')"
actual="$(shasum -a 256 "${dmg}" | awk '{print $1}')"
[ "${expected}" = "${actual}" ] || die "SHA-256 mismatch for ${dmg}"
printf 'Verified %s\n' "${dmg}"

MOUNTPOINT="$(mktemp -d "${TMPDIR}/ai-usage-dashboard-dmg.XXXXXX")"
hdiutil attach "${dmg}" -mountpoint "${MOUNTPOINT}" -nobrowse -quiet \
  || die "failed to mount ${dmg}"

app_src="$(find "${MOUNTPOINT}" -maxdepth 2 -name '*.app' -type d | head -n 1)"
[ -n "${app_src}" ] || die "no .app bundle found inside ${dmg}"

app_name="$(basename "${app_src}")"
mkdir -p "${APPLICATIONS_DIR}"
dest="${APPLICATIONS_DIR}/${app_name}"

# Prefer user Applications without sudo; fall back to /Applications with sudo.
if [ -w "${APPLICATIONS_DIR}" ] || mkdir -p "${APPLICATIONS_DIR}" 2>/dev/null; then
  rm -rf "${dest}"
  ditto "${app_src}" "${dest}"
else
  need_cmd sudo
  sudo rm -rf "${dest}"
  sudo ditto "${app_src}" "${dest}"
fi

printf 'Installed %s to %s\n' "${app_name}" "${dest}"
printf 'If macOS blocks the unsigned app: System Settings → Privacy & Security → Open Anyway\n'
printf 'Demo: open -a "%s" --args --judge-demo\n' "${app_name%.app}"
