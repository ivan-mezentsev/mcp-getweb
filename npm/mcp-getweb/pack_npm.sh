#!/usr/bin/env bash
# Stage and publish npm package using binaries from a GitHub Release.
#
# - Inputs:
#     - positional `version` (e.g. 0.1.0)
#     - flags `--dry-run` (bool), `--dir` (working/output dir; default: current working dir)
#     - flag `--repo <owner/name>` (REQUIRED) Instead of hardcoded repo
# - Behavior:
#     1) Download release assets for binaries `mcp-getweb-<target_triple>(.exe)` from tag `v<version>` via `gh release download`.
#     2) Copy binaries into `npm/mcp-getweb/bin/` (overwriting if present).
#     3) Run `npm pack` inside `npm/mcp-getweb`, produce `mcp-getweb-<version>.tgz`, then rename to `mcp-getweb-npm-<version>.tgz` into `--dir`.
#     4) Verify tarball content includes JS launcher and all binaries.
#     5) Run `npm publish [--dry-run] <tarball>` with `CI` removed from env.
# - Errors: tool missing, download failures, missing expected assets, pack/publish errors.
#
# Requirements:
# - MacOS: brew install gh node zstd unzip
# - GNU/Linux: sudo apt-get install -y gh nodejs npm tar unzip zstd

cd "$(dirname "$0")"

# Be strict: fail on any error, unset vars, and failed pipelines
set -euo pipefail

REPO=""
NPM_DIR="$(pwd)"
BIN_DIR="${NPM_DIR}/bin"

# Supported binary names (exact)
SUPPORTED_BINARIES=(
  "mcp-getweb-aarch64-apple-darwin"
  "mcp-getweb-x86_64-apple-darwin"
  "mcp-getweb-aarch64-unknown-linux-musl"
  "mcp-getweb-x86_64-unknown-linux-musl"
  "mcp-getweb-x86_64-pc-windows-msvc.exe"
)

# --- Utilities ---

err() { echo "$*" 1>&2; }

check_tool() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    err "Error: required tool '${name}' not found in PATH"
    exit 1
  fi
}

run() {
  # Run a command and exit on failure with message
  local cwd="" env_override=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --cwd)
        cwd="$2"; shift 2;;
      --env)
        # pass as KEY=VAL entries
        env_override+=("$2"); shift 2;;
      --)
        shift; break;;
      *)
        break;;
    esac
  done
  local -a cmd=("$@")
  if [[ -n "$cwd" ]]; then
    ( cd "$cwd" && env ${env_override[@]+"${env_override[@]}"} "${cmd[@]}" )
  else
    env ${env_override[@]+"${env_override[@]}"} "${cmd[@]}"
  fi
  local rc=$?
  if [[ $rc -ne 0 ]]; then
    err "Command failed with exit code ${rc}: ${cmd[*]}"
    exit "$rc"
  fi
}

run_capture() {
  # Capture stdout+stderr of a command, optional --cwd
  local cwd=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --cwd)
        cwd="$2"; shift 2;;
      --)
        shift; break;;
      *)
        break;;
    esac
  done
  local -a cmd=("$@")
  local out
  if [[ -n "$cwd" ]]; then
    out=$(cd "$cwd" && "${cmd[@]}" 2>&1)
  else
    out=$("${cmd[@]}" 2>&1)
  fi
  local rc=$?
  if [[ $rc -ne 0 ]]; then
    err "Command failed with exit code ${rc}: ${cmd[*]}"
    err "$out"
    exit "$rc"
  fi
  printf "%s" "$out"
}

ensure_executable() {
  # Ensure POSIX execute bit for non-Windows files
  local p="$1"
  if [[ "$p" != *.exe ]]; then
    chmod a+x "$p" 2>/dev/null || true
  fi
}

verify_launcher() {
  local launcher="${BIN_DIR}/mcp-getweb.js"
  if [[ ! -f "$launcher" ]]; then
    err "Error: launcher not found: $launcher"
    exit 1
  fi
}

strip_ext() {
  # Remove known archive extensions from filename, echo base
  local name="$1"
  case "$name" in
    *.tar.gz) echo "${name%.tar.gz}" ;;
    *.zip) echo "${name%.zip}" ;;
    *.zst) echo "${name%.zst}" ;;
    *) echo "$name" ;;
  esac
}

extract_asset() {
  # Extract or copy a downloaded asset into out_dir and echo the produced binary path; empty on failure
  # Supports: raw binaries, .zst single-file, .tar.gz, .zip
  local asset="$1" out_dir="$2"
  local filename base
  filename="$(basename -- "$asset")"
  base="$(strip_ext "$filename")"

  # raw binary
  if { [[ "$asset" != *.tar.gz && "$asset" != *.zip && "$asset" != *.zst ]] && [[ "$filename" == mcp-getweb-* ]]; } || [[ "$asset" == *.exe ]]; then
    local dest="${out_dir}/${filename}"
    rm -f -- "$dest"
    cp -p -- "$asset" "$dest"
    echo "$dest"
    return 0
  fi

  # .zst single-file
  if [[ "$asset" == *.zst ]]; then
    if ! command -v zstd >/dev/null 2>&1; then
      err "Warning: zstd not found; skipping $filename"
      echo ""
      return 0
    fi
    local dest="${out_dir}/${base}"
    run zstd -d "$asset" -o "$dest"
    [[ -f "$dest" ]] && echo "$dest" || echo ""
    return 0
  fi

  # .tar.gz
  if [[ "$asset" == *.tar.gz ]]; then
    if ! tar -tzf "$asset" >/dev/null 2>&1; then
      err "Warning: failed to inspect $filename"
      echo ""
      return 0
    fi
    if ! tar -xzf "$asset" -C "$out_dir" >/dev/null 2>&1; then
      err "Warning: failed to extract $filename"
      echo ""
      return 0
    fi
    local candidate="${out_dir}/${base}"
    if [[ -f "$candidate" ]]; then
      echo "$candidate"; return 0
    fi
    # fallback search
    local found
    found="$(find "$out_dir" -type f -name "$base" -print -quit 2>/dev/null || true)"
    echo "${found}"
    return 0
  fi

  # .zip
  if [[ "$asset" == *.zip ]]; then
    if ! command -v unzip >/dev/null 2>&1; then
      err "Warning: unzip not found; skipping $filename"
      echo ""
      return 0
    fi
    if ! unzip -qq -o "$asset" -d "$out_dir" >/dev/null 2>&1; then
      err "Warning: failed to unzip $filename"
      echo ""
      return 0
    fi
    local candidate="${out_dir}/${base}"
    if [[ -f "$candidate" ]]; then
      echo "$candidate"; return 0
    fi
    local found
    found="$(find "$out_dir" -type f -name "$base" -print -quit 2>/dev/null || true)"
    echo "${found}"
    return 0
  fi

  echo ""
}

usage() {
  echo "Usage: $0 --repo <owner/name> <version> [--dry-run] [--dir <path>]" 1>&2
}

main() {
  local version="" dry_run=0 download_dir="$(pwd)"

  # Parse args: first non-flag is version; flags --dry-run, --dir <path>
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --dry-run)
        dry_run=1; shift ;;
      --dir)
        [[ $# -ge 2 ]] || { err "Error: --dir requires a path"; usage; return 1; }
        download_dir="$2"; shift 2 ;;
      --repo)
        [[ $# -ge 2 ]] || { err "Error: --repo requires a value like 'owner/name'"; usage; return 1; }
        REPO="$2"; shift 2 ;;
      --help|-h)
        usage; return 0 ;;
      --*)
        err "Error: unknown option: $1"; usage; return 1 ;;
      *)
        if [[ -z "$version" ]]; then
          version="$1"; shift
        else
          err "Error: unexpected extra argument: $1"; usage; return 1
        fi ;;
    esac
  done

  check_tool gh
  check_tool npm

  if [[ -z "$REPO" ]]; then
    err "Error: --repo is required"
    return 1
  fi

  if [[ -z "$version" ]]; then
    err "Error: version must be non-empty"
    return 1
  fi

  local tag="${version}"
  local work_dir
  work_dir="$(mkdir -p -- "$download_dir" && cd -- "$download_dir" && pwd)"

  # Set npm version only if different to avoid 'Version not changed' error
  local current_version
  current_version=$(run_capture --cwd "$NPM_DIR" npm pkg get version | tr -d '"' | tr -d '\n' | tr -d ' ')
  if [[ "$current_version" != "$version" ]]; then
    echo "Setting npm package.json version to ${version}..."
    run --cwd "$NPM_DIR" npm version "$version"
  else
    echo "package.json already at version ${version}; skipping version update."
  fi

  # 1) Download all relevant assets in one go
  echo "Downloading release assets from '${tag}' in repo '${REPO}'..."
  run gh release download "$tag" --repo "$REPO" --dir "$work_dir" --pattern "mcp-getweb-*" #--skip-existing

  # 2) Stage into npm/mcp-getweb/bin
  verify_launcher
  mkdir -p -- "$BIN_DIR"
  local -a staged=()
  local tmp_dir
  tmp_dir="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf -- '$tmp_dir'" EXIT

  local asset produced dst name
  shopt -s nullglob
  for asset in "$work_dir"/mcp-getweb-*; do
    [[ -f "$asset" ]] || continue
    name="$(basename -- "$asset")"
    produced="$(extract_asset "$asset" "$tmp_dir")"
    if [[ -n "$produced" ]] && [[ "$(basename -- "$produced")" == mcp-getweb-* ]]; then
      dst="${BIN_DIR}/$(basename -- "$produced")"
      rm -f -- "$dst"
      mv -f -- "$produced" "$dst"
      ensure_executable "$dst"
      staged+=("$(basename -- "$dst")")
    fi
  done
  shopt -u nullglob

  # Warn on missing expected targets, but do not fail
  if [[ ${#staged[@]} -gt 0 ]]; then
    local -a missing=()
    local i found
    for i in "${SUPPORTED_BINARIES[@]}"; do
      found=0
      for name in "${staged[@]}"; do
        if [[ "$name" == "$i" ]]; then found=1; break; fi
      done
      if [[ $found -eq 0 ]]; then missing+=("$i"); fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
      err "Warning: missing targets (skipped): ${missing[*]}"
    fi
  fi
  if [[ ${#staged[@]} -eq 0 ]]; then
    err "Error: no binaries were staged for packaging"
    return 1
  fi

  # 3) npm pack and rename to mcp-getweb-npm-<version>.tgz in work_dir
  echo "Packing npm tarball..."
  # Ensure README.md from repo root is included in the npm package
  if [[ -f "$NPM_DIR/../../README.md" ]]; then
    cp -p -- "$NPM_DIR/../../README.md" "$NPM_DIR/README.md"
  fi
  local produced_line produced_path dest_tar
  produced_line=$( (cd "$NPM_DIR" && npm pack --silent) | tail -n 1 )
  produced_path="${NPM_DIR}/${produced_line}"
  if [[ ! -f "$produced_path" ]]; then
    err "Error: npm pack did not produce expected file: $produced_path"
    return 1
  fi

  dest_tar="${work_dir}/mcp-getweb-npm-${version}.tgz"
  rm -f -- "$dest_tar"
  mv -f -- "$produced_path" "$dest_tar"

  # 4) Verify tarball composition
  local tar_list
  tar_list="$(tar tzf "$dest_tar")"
  local required
  required=("package/bin/mcp-getweb.js")
  for name in "${staged[@]}"; do
    required+=("package/bin/${name}")
  done
  local missing_inside=()
  local req
  for req in "${required[@]}"; do
    if ! grep -Fx -- "$req" <<<"$tar_list" >/dev/null 2>&1; then
      missing_inside+=("$req")
    fi
  done
  if [[ ${#missing_inside[@]} -gt 0 ]]; then
    err "Error: tarball is missing entries: ${missing_inside[*]}"
    return 1
  fi

  # 5) Publish (or dry-run) with CI removed from env
  local -a npm_cmd=("npm" "publish")
  if [[ $dry_run -eq 1 ]]; then
    npm_cmd+=("--dry-run")
  fi
  npm_cmd+=("$dest_tar")
  echo "Publishing to npm"$([[ $dry_run -eq 1 ]] && echo " (dry-run)" || true)": $(basename -- "$dest_tar")"
  # Run publish and fail the script if it errors
  ( cd "$work_dir" && env -u CI "${npm_cmd[@]}" ) || { err "Error: npm publish failed"; return 1; }

  echo "Pack & publish completed successfully."
  return 0
}

main "$@"
