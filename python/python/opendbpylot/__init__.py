"""opendbpylot — natural-language → SQL via Retrieval-Augmented Generation.

Example
-------
>>> import opendbpylot
>>> bot = opendbpylot.OpenDbPylot()          # uses config from `dbpylot init`
>>> result = bot.ask("how many orders per country?")
>>> print(result["sql"])
>>> for row in result["rows"]:
...     print(row)

Run the setup wizard first (needs the `dbpylot` CLI: `cargo install opendbpylot`):
>>> opendbpylot.OpenDbPylot.init()
"""

from ._native import OpenDbPylot, __version__

__all__ = ["OpenDbPylot", "__version__"]
