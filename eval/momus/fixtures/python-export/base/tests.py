from pathlib import Path
from exports import resolve_export
assert resolve_export(Path("/exports"), "report.csv") == Path("/exports/report.csv")
