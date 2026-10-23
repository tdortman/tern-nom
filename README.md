# tern-nom

[nix-output-monitor](https://github.com/maralorn/nix-output-monitor) (`nom`) for the [Tern](https://stencil.so/tern) terminal.
In a Tern pane, `nom build` and anything that pipes into `nom --json`, such as `nh os switch`, show a native card instead of nom's text table.
The card shows running builds, downloads, errors, and the build log.
Everywhere else it runs the real `nom`, so nothing changes outside Tern.

## What the card shows

- Built and fetched counts, an overall progress bar, and a timer.
- One row per running build, with its stdenv phase, its last output line, and how long it has run.
- One row per download, with a byte progress bar.
- Errors and warnings as nix prints them.
- The finished builds, failed ones first. The section folds when nothing failed.
- The tail of the build log, which folds once the build succeeds.

Ctrl+C stops nix as usual. The card then reads Cancelled and `nom` exits with 130.

## Install

The flake's default package installs `nom`, plus `nom-build` and `nom-shell` from nix-output-monitor.
Add it as a flake input:

```nix
inputs.tern-nom = {
	url = "github:tdortman/tern-nom";
	inputs.nixpkgs.follows = "nixpkgs";
};
```

Put `inputs.tern-nom.packages.${system}.default` in your packages instead of `nix-output-monitor`.

nh puts its own copy of nom on its `PATH`. To make nh use tern-nom, override nh:

```nix
pkgs.nh.override { nix-output-monitor = inputs.tern-nom.packages.${system}.default; }
```

## Which commands draw a card

| Command | In a Tern pane |
| --- | --- |
| `nom build`, `nom copy`, `nom flake` | Card. tern-nom runs `nix` with `-v --log-format internal-json`. |
| `nom --json` | Card. tern-nom reads nix's log from stdin. nh runs nom this way. |
| `nom shell`, `nom develop`, plain `nom`, `nom-build`, `nom-shell` | The real `nom`. |

The card draws only if all of these are true:

- `TERN_PANE` or `TERM_PROGRAM=tern` is set.
- `TERN_TSP` is not `0`.
- tmux, screen, and zellij are not running.
- Tern answers the surface-protocol handshake on `/dev/tty`.

Otherwise tern-nom runs the binary named by `TERN_NOM_REAL` with the same arguments. The package sets `TERN_NOM_REAL` to nix-output-monitor's `nom`.

ssh does not forward `TERN_PANE`, so builds on a remote host get the plain `nom` table.

## Develop

```sh
nix develop
cargo nextest run
nix fmt
```

`tests/` holds two captured Lix logs, one build that succeeds and one that fails. The tests replay them through the parser.

## License

MIT
