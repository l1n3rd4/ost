{ pkgs ? (import (builtins.fetchTarball {
  url = "https://github.com/nixos/nixpkgs/tarball/25.05";
  sha256 = "1915r28xc4znrh2vf4rrjnxldw2imysz819gzhk9qlrkqanmfsxd";
}) {}) }:

pkgs.mkShell {
  name = "teams-cli";

  buildInputs = with pkgs; [
    # General tools
    just
    jq
    ripgrep
    fd
    gh

    # Rust development
    rustc
    cargo
    rust-analyzer
    clippy
    rustfmt

    # Build tools
    gnumake
    cmake
    pkg-config
    openssl
  ];

  shellHook = ''
    export PKG_CONFIG_PATH="${pkgs.openssl.dev}/lib/pkgconfig:$PKG_CONFIG_PATH"
  '';
}
