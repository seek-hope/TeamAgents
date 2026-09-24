#!/bin/sh
# Release bootstrap only; product behavior lives in the Rust crates.
set -eu

repo=seek-hope/TeamAgents
version=
archive=
bin_dir=${TEAMAGENTS_BIN_DIR:-${HOME:?HOME must be set}/.local/bin}

fail() { printf 'install failed: %s\n' "$*" >&2; exit 1; }
usage() {
    cat <<'EOF'
TeamAgents installer (Linux x86_64)
usage: sh install.sh [--version VERSION] [--bin-dir DIR] [--archive FILE]
  downloads the latest release, verifies SHA-256 and installs teamagents and teamagents-tui.
  --version VERSION  pick a version, e.g. 0.1.1 or v0.1.1
  --bin-dir DIR      install directory (default ~/.local/bin; no sudo needed)
  --archive FILE     use a local release archive with its SHA256SUMS beside it (no network)
  --help            print this help
A private repository needs `gh auth login` with an account that can read it.
Quit a running TeamAgents before upgrading. The existing config and sessions are kept.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --version|--bin-dir|--archive)
            [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$1 needs an argument"
            case "$1" in
                --version) version=${2#v} ;;
                --bin-dir) bin_dir=$2 ;;
                --archive) archive=$2 ;;
            esac
            shift 2 ;;
        *) fail "unknown argument $1; use --help" ;;
    esac
done

[ "$(uname -s)" = Linux ] || fail "only Linux x86_64 is supported"
case "$(uname -m)" in x86_64|amd64) ;; *) fail "only Linux x86_64 release archives are published" ;; esac
for tool in mktemp tar sha256sum install awk cp mv; do
    command -v "$tool" >/dev/null 2>&1 || fail "missing command $tool"
done

temp_dir=$(mktemp -d)
stage_dir=
install_started=0
install_done=0
cleanup() {
    status=$?
    if [ "$install_started" = 1 ] && [ "$install_done" = 0 ]; then
        for binary in teamagents teamagents-tui; do
            if [ -e "$stage_dir/backup-$binary" ] || [ -L "$stage_dir/backup-$binary" ]; then
                mv -f "$stage_dir/backup-$binary" "$bin_dir/$binary" || {
                    printf 'restore failed; the previous binaries are kept in %s\n' "$stage_dir" >&2
                    exit 1
                }
            else
                rm -f "$bin_dir/$binary"
            fi
        done
    fi
    [ -z "$stage_dir" ] || rm -rf "$stage_dir"
    rm -rf "$temp_dir"
    exit "$status"
}
trap cleanup 0
trap 'exit 1' 1 2 15

download() {
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time 300 --retry 2 "$@"
}

if [ -n "$archive" ]; then
    [ -f "$archive" ] || fail "release archive not found: $archive"
    name=$(basename "$archive")
    case "$name" in
        teamagents-*-x86_64-unknown-linux-musl.tar.gz)
            archive_version=${name#teamagents-}
            archive_version=${archive_version%-x86_64-unknown-linux-musl.tar.gz} ;;
        *) fail "the archive name is not a Linux x86_64 release name: $name" ;;
    esac
    [ -z "$version" ] || [ "$version" = "$archive_version" ] || fail "--version does not match the local archive version"
    version=$archive_version
    cp "$archive" "$temp_dir/$name"
    cp "$(dirname "$archive")/SHA256SUMS" "$temp_dir/SHA256SUMS" || fail "SHA256SUMS is missing next to the local archive"
else
    transport=curl
    if command -v gh >/dev/null 2>&1 && gh auth status --hostname github.com >/dev/null 2>&1; then
        transport=gh
    else
        command -v curl >/dev/null 2>&1 || fail "install curl; for a private repository also install gh and run gh auth login"
    fi
    if [ -z "$version" ]; then
        if [ "$transport" = gh ]; then
            tag=$(gh release view --repo "$repo" --json tagName --jq .tagName) || fail "cannot list releases; check the network and repository access"
        else
            url=$(download --output /dev/null --write-out '%{url_effective}' "https://github.com/$repo/releases/latest") \
                || fail "cannot list releases; for a private repository install gh and run gh auth login"
            case "$url" in
                "https://github.com/$repo/releases/tag/"*) tag=${url##*/} ;;
                *) fail "no release found; check repository access and the gh login state" ;;
            esac
        fi
        version=${tag#v}
    fi
fi

case "$version" in ''|*[!a-zA-Z0-9.+-]*|[!0-9]*) fail "invalid version: $version" ;; esac
package=teamagents-$version-x86_64-unknown-linux-musl
name=$package.tar.gz
if [ -z "$archive" ]; then
    printf 'downloading TeamAgents v%s...\n' "$version"
    if [ "$transport" = gh ]; then
        gh release download "v$version" --repo "$repo" --pattern "$name" --pattern SHA256SUMS --dir "$temp_dir" \
            || fail "download failed; check the network, the version and repository access"
    else
        base=https://github.com/$repo/releases/download/v$version
        download --output "$temp_dir/$name" "$base/$name" || fail "archive download failed; for a private repository run gh auth login and retry"
        download --output "$temp_dir/SHA256SUMS" "$base/SHA256SUMS" || fail "checksum file download failed"
    fi
fi

# Select exactly this archive: future multi-platform manifests may list other files.
awk -v name="$name" '$2 == name || $2 == "*" name { print; count++ } END { if (count != 1) exit 1 }' \
    "$temp_dir/SHA256SUMS" > "$temp_dir/selected.sha256" || fail "SHA256SUMS does not list $name exactly once"
(cd "$temp_dir" && sha256sum -c selected.sha256) || fail "SHA-256 verification failed; installed binaries were left untouched"
tar -xzf "$temp_dir/$name" -C "$temp_dir" \
    "$package/teamagents" "$package/teamagents-tui" "$package/config.example.toml" || fail "the release archive is incomplete"
for file in teamagents teamagents-tui config.example.toml; do
    [ -f "$temp_dir/$package/$file" ] && [ ! -L "$temp_dir/$package/$file" ] || fail "invalid archive content: $file"
done

mkdir -p "$bin_dir"
bin_dir=$(cd "$bin_dir" && pwd)
stage_dir=$(mktemp -d "$bin_dir/.teamagents-install.XXXXXX")
for binary in teamagents teamagents-tui; do
    [ ! -d "$bin_dir/$binary" ] || fail "the target is a directory: $bin_dir/$binary"
    install -m755 "$temp_dir/$package/$binary" "$stage_dir/$binary"
    if [ -e "$bin_dir/$binary" ] || [ -L "$bin_dir/$binary" ]; then
        cp -Pp "$bin_dir/$binary" "$stage_dir/backup-$binary"
    fi
done
# Rename staged files so upgrading a running executable never truncates it.
install_started=1
for binary in teamagents teamagents-tui; do
    mv -f "$stage_dir/$binary" "$bin_dir/$binary"
done
install_done=1
printf 'installed TeamAgents v%s into %s\n' "$version" "$bin_dir"

# v0.1.1 predates init; keep the bootstrap usable before the next release.
help_text=$("$bin_dir/teamagents" --help 2>&1 || true)
case "$help_text" in
    *'teamagents init'*) printf 'next: teamagents init\n' ;;
    *)
        config_path=${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml
        if [ -e "$config_path" ] || [ -L "$config_path" ]; then
            printf 'kept the existing config: %s\n' "$config_path"
        else
            mkdir -p "$(dirname "$config_path")"
            (umask 077; set -C; cat "$temp_dir/$package/config.example.toml" > "$config_path")
            printf 'this release has no init command yet; wrote a config: %s\n' "$config_path"
        fi
        printf 'next: set the credential env var named in the config, then run teamagents doctor.\n' ;;
esac
if ! command -v bwrap >/dev/null 2>&1; then
    printf 'also install bubblewrap: Debian/Ubuntu use sudo apt install bubblewrap; Fedora uses sudo dnf install bubblewrap.\n'
fi
case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *)
        # Quote custom directories safely when printing a command to copy.
        quoted_dir=$(printf '%s' "$bin_dir" | sed "s/'/'\\\\''/g")
        printf "add this to ~/.bashrc or ~/.zshrc:\nexport PATH='%s':\"\$PATH\"\n" "$quoted_dir" ;;
esac
printf 'start: run teamagents in your project directory; to upgrade, quit it and rerun this script.\n'
