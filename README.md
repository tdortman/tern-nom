# tern-nom

[nix-output-monitor](https://github.com/maralorn/nix-output-monitor) (`nom`) for the [Tern](https://stencil.so/tern) terminal.
In a Tern pane, `nom build` and anything that pipes into `nom --json`, such as `nh os switch`, show a native card instead of nom's text table.
The card shows the dependency tree, running builds with estimated progress, downloads, errors, and the build log.
Everywhere else it runs the real `nom`, so nothing changes outside Tern.

## What the card shows

- Built and fetched counts, what nix is doing, and a timer.
- At the bottom, a progress bar for builds and a download bar over all paths fetched from caches.
- One row per running build, with its stdenv phase, its last output line, and how long it has run.
  If nom has timed the same package before, the row also has a progress bar against that estimate.
- One row per path being fetched, with its own download bar.
  A build or fetch gets its row once it has run for a second, so the many that finish sooner do not flash in and out.
- A failed build as its name, exit code, its own last log lines, and the `nix log` command.
  Other errors show as nix prints them, with store paths cut down to the package name.
- The derivations nix builds, as a tree from what you asked for down to its inputs, in a fixed order.
  Each one shows whether it is waiting, building, built, or failed, with its time.
  Subtrees stay folded unless something in them failed, and you can open or fold any of them.
- Warnings in a folded section, one line each.
  A warning nix prints several times shows once with a count, store paths are cut down to the package name, and quoted option names show as code.
- The paths fetched from caches, in a folded section.
- Build logs stay hidden. Click a running build's row to show the last 8 lines of its log under the row, and click again to hide it.
  The button above those lines expands them to the last 200 lines, in a box that scrolls.
  A failed build's log is always shown in its failure block.
  Output nix prints outside any build sits in a folded section.

Ctrl+C stops nix as usual. The card then reads Cancelled and `nom` exits with 130.

## Build-time estimates

tern-nom reads and writes nom's own history, `$XDG_STATE_HOME/nix-output-monitor/build-reports.csv`.
It uses the same columns, lock file, and limit of 10 runs per package as nom.
The estimate for a build is the median of its recorded runs, as in nom.
Your existing nom history gives estimates from the first run, and builds timed by either program improve both.

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

`tests/` holds two captured Lix logs, one build that succeeds and one that fails.
The tests replay them through the parser and check the counts, the dependency tree, and which build failed.

## License

MIT
