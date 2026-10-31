"""Router hook that calls the engine's topological router (`topor_engine.route`); the DRC-spots
feedback of the bench loop lives in toporlib.drcloop (re-exported here)."""
from __future__ import annotations

import json

from toporlib.drcloop import _layer_of, drc_spots  # noqa: F401  (re-export)


def route_router(model: dict, options: dict) -> dict:
    """Run the router. Returns {"items": [...], "remove_uuids": [], "report": <RouteResult dict>}."""
    import topor_engine

    out = json.loads(topor_engine.route(json.dumps(model), json.dumps(options)))
    return {"items": list(out.get("items") or []), "remove_uuids": [], "report": out}
