#!/usr/bin/env sh
# meshh installer
# Usage: curl -fsSL https://raw.githubusercontent.com/meshhai/meshh-tui/master/scripts/install.sh | sh

set -eu

repo="${MESHH_REPO:-meshhai/meshh-tui}"
binary="meshh"

info() {
  printf '%s\n' "meshh: $*"
}

error() {
  printf '%s\n' "meshh: error: $*" >&2
  exit 1
}

need_command() {
  command -v "$1" >/dev/null 2>&1 || error "missing required command: $1"
}

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux) os_part="unknown-linux-gnu" ;;
    *) error "unsupported operating system: $os" ;;
  esac

  case "$arch" in
    arm64 | aarch64) arch_part="aarch64" ;;
    x86_64 | amd64) arch_part="x86_64" ;;
    *) error "unsupported architecture: $arch" ;;
  esac

  printf '%s-%s\n' "$arch_part" "$os_part"
}

install_dir() {
  if [ -n "${MESHH_INSTALL_DIR:-}" ]; then
    printf '%s\n' "$MESHH_INSTALL_DIR"
  elif [ -d "$HOME/.local/bin" ]; then
    printf '%s\n' "$HOME/.local/bin"
  else
    printf '%s\n' "/usr/local/bin"
  fi
}

download() {
  url="$1"
  output="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$output"
  else
    error "curl or wget is required to download releases"
  fi
}

latest_version() {
  url="https://github.com/${repo}/releases/latest"

  if command -v curl >/dev/null 2>&1; then
    final_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$url")"
  elif command -v wget >/dev/null 2>&1; then
    final_url="$(wget -qS --spider "$url" 2>&1 | awk '/^  Location: / {print $2}' | tail -n 1 | tr -d '\r')"
  else
    error "curl or wget is required to resolve the latest release"
  fi

  case "$final_url" in
    */releases/tag/*) printf '%s\n' "${final_url##*/releases/tag/}" ;;
    *) error "could not resolve latest Meshh release" ;;
  esac
}

verify_checksum() {
  archive="$1"
  checksum_file="$2"

  if [ "${MESHH_SKIP_CHECKSUM:-}" = "1" ]; then
    info "skipping checksum verification because MESHH_SKIP_CHECKSUM=1"
    return
  fi

  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "$checksum_file"
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "$checksum_file"
  else
    error "shasum or sha256sum is required to verify $archive"
  fi
}

install_binary() {
  source_path="$1"
  dest_dir="$2"

  mkdir -p "$dest_dir" 2>/dev/null || true

  if [ -w "$dest_dir" ]; then
    install -m 0755 "$source_path" "$dest_dir/$binary"
  elif command -v sudo >/dev/null 2>&1; then
    sudo mkdir -p "$dest_dir"
    sudo install -m 0755 "$source_path" "$dest_dir/$binary"
  else
    error "$dest_dir is not writable and sudo is unavailable"
  fi
}

main() {
  need_command uname
  need_command tar
  need_command install
  need_command mktemp

  target="$(detect_target)"
  version="${MESHH_VERSION:-$(latest_version)}"
  version_number="${version#v}"
  archive_name="meshh_${version_number}_${target}.tar.gz"
  base_url="https://github.com/${repo}/releases/download/${version}"
  dest_dir="$(install_dir)"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT INT TERM

  info "installing $binary $version for $target"
  info "install directory: $dest_dir"

  download "$base_url/$archive_name" "$tmpdir/$archive_name"
  download "$base_url/$archive_name.sha256" "$tmpdir/$archive_name.sha256"
  (
    cd "$tmpdir"
    verify_checksum "$archive_name" "$archive_name.sha256"
    tar -xzf "$archive_name"
  )

  install_binary "$tmpdir/meshh_${version_number}_${target}/$binary" "$dest_dir"

  info "installed $("$dest_dir/$binary" --version)"
  if ! echo ":$PATH:" | grep -q ":$dest_dir:"; then
    info "$dest_dir is not on PATH"
  fi
  info "next: meshh login"
}

main "$@"
