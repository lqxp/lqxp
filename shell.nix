{ pkgs ? import <nixpkgs> {} }:

let
  inherit (pkgs) lib stdenv;
  nodejs = pkgs.nodejs_24 or pkgs.nodejs_22;
in
pkgs.mkShell {
  packages = [
    nodejs
    pkgs.rustup
    pkgs.pkg-config
    pkgs.openssl
    pkgs.git
  ] ++ lib.optionals stdenv.isLinux [
    pkgs.cairo
    pkgs.dbus
    pkgs.gdk-pixbuf
    pkgs.glib
    pkgs.gtk3
    pkgs.librsvg
    pkgs.libsoup_3
    pkgs.webkitgtk_4_1
    pkgs.libx11
    pkgs.libxcursor
    pkgs.libxi
    pkgs.libxrandr
  ] ++ lib.optionals stdenv.isDarwin [
    pkgs.cocoapods
  ];

  shellHook = ''
    export npm_config_prefix="$PWD/.npm-global"
    export PATH="$PWD/.npm-global/bin:$PATH"

    alias qxp-ios-build="./scripts/build-ios-local.sh"
    alias qxp-ios-dev="./scripts/dev-ios-local.sh"

    echo "QxProtocol dev shell ready."
    echo "Use qxp-ios-build on macOS/Xcode to build the Tauri iOS app."
    if [ "$(uname -s)" != "Darwin" ]; then
      echo "Note: iOS compilation itself is only available on macOS with Xcode."
    fi
  '';
}
