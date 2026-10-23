{
  lib,
  stdenv,
  inputs,
  makeBinaryWrapper,
  nh,
  symlinkJoin,
  ...
}:
let
  real = nh.override { nix-output-monitor = tern-nom; };
  tern-nom = inputs.self.packages.${stdenv.hostPlatform.system}.tern-nom;
in
# nh using tern-nom, without nh's own text diff where the card draws it.
symlinkJoin {
  nativeBuildInputs = [ makeBinaryWrapper ];

  postBuild = ''
    rm $out/bin/nh
    makeWrapper ${lib.getExe' tern-nom "nom"} $out/bin/nh --add-flags "--nh ${lib.getExe real}"
  '';

  name = "nh-tern-${real.version}";
  paths = [ real ];

  meta = real.meta // {
    mainProgram = "nh";
  };
}
