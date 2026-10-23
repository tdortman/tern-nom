{ pkgs, ... }:
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    cargo
    cargo-nextest
    clippy
    nix-output-monitor
    rust-analyzer
    rustc
    rustfmt
  ];

  # `cargo run -- build …` falls back to this nom outside Tern.
  shellHook = ''
    export TERN_NOM_REAL=${pkgs.lib.getExe' pkgs.nix-output-monitor "nom"}
  '';
}
