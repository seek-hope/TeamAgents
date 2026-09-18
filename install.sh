#!/bin/sh
# Release bootstrap only; product behavior lives in the Rust crates.
set -eu

repo=seek-hope/TeamAgents
version=
archive=
bin_dir=${TEAMAGENTS_BIN_DIR:-${HOME:?请设置 HOME}/.local/bin}

fail() { printf '安装失败：%s\n' "$*" >&2; exit 1; }
usage() {
    cat <<'EOF'
TeamAgents 安装程序（Linux x86_64）
用法：sh install.sh [--version VERSION] [--bin-dir DIR] [--archive FILE]
  默认下载最新发行版，校验 SHA-256 并安装 teamagents 与 teamagents-tui。
  --version VERSION  指定版本，例如 0.1.1 或 v0.1.1
  --bin-dir DIR      安装目录（默认 ~/.local/bin；无需 sudo）
  --archive FILE     使用本地发行包，旁边需有配套 SHA256SUMS（不联网）
  --help            查看帮助
私有仓库需要先用具有仓库访问权限的账号执行 gh auth login。
升级前请退出正在运行的 TeamAgents。已有配置与会话会保留。
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --version|--bin-dir|--archive)
            [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$1 缺少参数"
            case "$1" in
                --version) version=${2#v} ;;
                --bin-dir) bin_dir=$2 ;;
                --archive) archive=$2 ;;
            esac
            shift 2 ;;
        *) fail "未知参数 $1；用 --help 查看用法" ;;
    esac
done

[ "$(uname -s)" = Linux ] || fail "目前只支持 Linux x86_64"
case "$(uname -m)" in x86_64|amd64) ;; *) fail "目前只提供 Linux x86_64 发行包" ;; esac
for tool in mktemp tar sha256sum install awk cp mv; do
    command -v "$tool" >/dev/null 2>&1 || fail "缺少命令 $tool"
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
                    printf '恢复失败，旧程序保留在 %s\n' "$stage_dir" >&2
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
    [ -f "$archive" ] || fail "发行包不存在：$archive"
    name=$(basename "$archive")
    case "$name" in
        teamagents-*-x86_64-unknown-linux-musl.tar.gz)
            archive_version=${name#teamagents-}
            archive_version=${archive_version%-x86_64-unknown-linux-musl.tar.gz} ;;
        *) fail "发行包文件名不符合 Linux x86_64 格式：$name" ;;
    esac
    [ -z "$version" ] || [ "$version" = "$archive_version" ] || fail "--version 与本地发行包版本不一致"
    version=$archive_version
    cp "$archive" "$temp_dir/$name"
    cp "$(dirname "$archive")/SHA256SUMS" "$temp_dir/SHA256SUMS" || fail "本地发行包旁缺少 SHA256SUMS"
else
    transport=curl
    if command -v gh >/dev/null 2>&1 && gh auth status --hostname github.com >/dev/null 2>&1; then
        transport=gh
    else
        command -v curl >/dev/null 2>&1 || fail "请安装 curl；私有仓库请安装 gh 并执行 gh auth login"
    fi
    if [ -z "$version" ]; then
        if [ "$transport" = gh ]; then
            tag=$(gh release view --repo "$repo" --json tagName --jq .tagName) || fail "无法查询发行版，请检查网络及仓库访问权限"
        else
            url=$(download --output /dev/null --write-out '%{url_effective}' "https://github.com/$repo/releases/latest") \
                || fail "无法查询发行版；私有仓库请先安装 gh 并执行 gh auth login"
            case "$url" in
                "https://github.com/$repo/releases/tag/"*) tag=${url##*/} ;;
                *) fail "未找到最新发行版；请检查仓库访问权限及 gh 登录状态" ;;
            esac
        fi
        version=${tag#v}
    fi
fi

case "$version" in ''|*[!a-zA-Z0-9.+-]*|[!0-9]*) fail "无效版本号：$version" ;; esac
package=teamagents-$version-x86_64-unknown-linux-musl
name=$package.tar.gz
if [ -z "$archive" ]; then
    printf '正在下载 TeamAgents v%s…\n' "$version"
    if [ "$transport" = gh ]; then
        gh release download "v$version" --repo "$repo" --pattern "$name" --pattern SHA256SUMS --dir "$temp_dir" \
            || fail "下载失败，请检查网络、版本号及仓库权限"
    else
        base=https://github.com/$repo/releases/download/v$version
        download --output "$temp_dir/$name" "$base/$name" || fail "发行包下载失败；私有仓库请执行 gh auth login 后重试"
        download --output "$temp_dir/SHA256SUMS" "$base/SHA256SUMS" || fail "校验文件下载失败"
    fi
fi

# Select exactly this archive: future multi-platform manifests may list other files.
awk -v name="$name" '$2 == name || $2 == "*" name { print; count++ } END { if (count != 1) exit 1 }' \
    "$temp_dir/SHA256SUMS" > "$temp_dir/selected.sha256" || fail "SHA256SUMS 未唯一列出 $name"
(cd "$temp_dir" && sha256sum -c selected.sha256) || fail "SHA-256 校验失败，未修改已安装程序"
tar -xzf "$temp_dir/$name" -C "$temp_dir" \
    "$package/teamagents" "$package/teamagents-tui" "$package/config.example.toml" || fail "发行包不完整"
for file in teamagents teamagents-tui config.example.toml; do
    [ -f "$temp_dir/$package/$file" ] && [ ! -L "$temp_dir/$package/$file" ] || fail "发行包内容无效：$file"
done

mkdir -p "$bin_dir"
bin_dir=$(cd "$bin_dir" && pwd)
stage_dir=$(mktemp -d "$bin_dir/.teamagents-install.XXXXXX")
for binary in teamagents teamagents-tui; do
    [ ! -d "$bin_dir/$binary" ] || fail "目标是目录：$bin_dir/$binary"
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
printf '已安装 TeamAgents v%s 到 %s\n' "$version" "$bin_dir"

# v0.1.1 predates init; keep the bootstrap usable before the next release.
help_text=$("$bin_dir/teamagents" --help 2>&1 || true)
case "$help_text" in
    *'teamagents init'*) printf '下一步：teamagents init\n' ;;
    *)
        config_path=${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml
        if [ -e "$config_path" ] || [ -L "$config_path" ]; then
            printf '已保留现有配置：%s\n' "$config_path"
        else
            mkdir -p "$(dirname "$config_path")"
            (umask 077; set -C; cat "$temp_dir/$package/config.example.toml" > "$config_path")
            printf '此版本尚无 init 命令，已创建配置：%s\n' "$config_path"
        fi
        printf '下一步：按配置设置密钥环境变量，再运行 teamagents doctor。\n' ;;
esac
if ! command -v bwrap >/dev/null 2>&1; then
    printf '还需安装 bubblewrap：Debian/Ubuntu 用 sudo apt install bubblewrap；Fedora 用 sudo dnf install bubblewrap。\n'
fi
case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *)
        # Quote custom directories safely when printing a command to copy.
        quoted_dir=$(printf '%s' "$bin_dir" | sed "s/'/'\\\\''/g")
        printf "请执行以下命令，并加入 ~/.bashrc 或 ~/.zshrc：\nexport PATH='%s':\"\$PATH\"\n" "$quoted_dir" ;;
esac
printf '启动：在项目目录运行 teamagents；升级时退出程序后重新运行本脚本。\n'
