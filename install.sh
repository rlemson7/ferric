#!/usr/bin/env bash
# Build, bundle, and install the Ferric plugin into the user's
# audio plug-in folders. macOS only.

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This installer targets macOS. Adapt paths for Linux/Windows." >&2
    exit 1
fi

PLUGIN="ferric"
VST3_DIR="$HOME/Library/Audio/Plug-Ins/VST3"
CLAP_DIR="$HOME/Library/Audio/Plug-Ins/CLAP"

if ! command -v cargo-nih-plug >/dev/null 2>&1; then
    cat >&2 <<'EOF'
cargo-nih-plug not found. Install it once with:

    cargo install --git https://github.com/robbert-vdh/nih-plug.git cargo-nih-plug
EOF
    exit 1
fi

echo "==> Bundling ${PLUGIN} (release)"
cargo nih-plug bundle "${PLUGIN}" --release

VST3_SRC="target/bundled/${PLUGIN}.vst3"
CLAP_SRC="target/bundled/${PLUGIN}.clap"

if [[ ! -d "${VST3_SRC}" && ! -d "${CLAP_SRC}" ]]; then
    echo "No bundle artifacts found in target/bundled/." >&2
    exit 1
fi

mkdir -p "${VST3_DIR}" "${CLAP_DIR}"

if [[ -d "${VST3_SRC}" ]]; then
    echo "==> Installing VST3 -> ${VST3_DIR}/${PLUGIN}.vst3"
    rm -rf "${VST3_DIR:?}/${PLUGIN}.vst3"
    cp -R "${VST3_SRC}" "${VST3_DIR}/"
    xattr -dr com.apple.quarantine "${VST3_DIR}/${PLUGIN}.vst3" 2>/dev/null || true
fi

if [[ -d "${CLAP_SRC}" ]]; then
    echo "==> Installing CLAP -> ${CLAP_DIR}/${PLUGIN}.clap"
    rm -rf "${CLAP_DIR:?}/${PLUGIN}.clap"
    cp -R "${CLAP_SRC}" "${CLAP_DIR}/"
    xattr -dr com.apple.quarantine "${CLAP_DIR}/${PLUGIN}.clap" 2>/dev/null || true
fi

echo "Done. Rescan plugins in your DAW if needed."
