from pathlib import Path
from exports import resolve_export
assert resolve_export(Path("/exports"), "month/report.csv") == Path("/exports/month/report.csv")
