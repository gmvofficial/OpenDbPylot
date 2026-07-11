"""Console-script entry point for `dbpylot`.

Registered in pyproject.toml so `pip install opendbpylot` puts a fully working
`dbpylot` command on PATH — it runs the real CLI **in-process** via the compiled
native module, so no separate Rust binary is required.
"""

from __future__ import annotations

import sys


def main() -> None:
    from ._native import run_cli

    # Pass a stable program name as argv[0] so help/usage reads "dbpylot".
    raise SystemExit(run_cli(["dbpylot", *sys.argv[1:]]))


if __name__ == "__main__":
    main()
