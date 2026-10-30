"""Router hook that calls the engine's smoother (`topor_engine.smooth`) on hand copper."""
from __future__ import annotations

import json


def smooth_router(model: dict, options: dict) -> dict:
    """Run the smoother; forces existing_copper="smooth".

    Returns {"items": [...], "remove_uuids": [...], "report": <engine report dict>}.
    """
    import topor_engine

    opts = {**options, "existing_copper": "smooth"}
    out = json.loads(topor_engine.smooth(json.dumps(model), json.dumps(opts)))
    return {
        "items": list(out.get("items") or []),
        "remove_uuids": list(out.get("replaced") or []),
        "report": out,
    }
