"""Put the vendored project root on sys.path.

Upstream, `kalshi_mention/` sat directly under the DelphiResearch repo root and
pytest's rootdir insertion made `import kalshi_mention` work for free. Here the
package is one level deeper, under `kalshi-mention/`, so pytest needs this file
to prepend that directory. Its presence is what lets

    python -m pytest kalshi-mention/tests -q

run from the repo root.
"""

import sys
from pathlib import Path

VENDOR_ROOT = Path(__file__).resolve().parent
if str(VENDOR_ROOT) not in sys.path:
    sys.path.insert(0, str(VENDOR_ROOT))
