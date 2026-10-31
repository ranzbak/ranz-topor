"""Stackup roles (46 row 13, M4-R7): which copper layer is routed and which is a plane.

Finding codes produced here (B6 adds them to messages.py / troubleshoot.md):
    LAYER_ROLES_NEEDED  (error)  copper count has no default stackup and config roles do not cover every layer.

Roles: signal | mixed | jumper | gnd | pwr | power. ``power`` = a KiCad "power" layer type whose expected
net is a ground OR a power net. Routed iff signal or mixed.

Precedence per layer: (1) ``[layers]`` roles in topor.toml ("config"); (2) KiCad layer types, used when ANY
copper layer is power/mixed/jumper ("kicad"); (3) defaults by copper count ("default").

Complexity: O(L) in the number of copper layers.
"""

from __future__ import annotations

from .findings import Finding

ROLES = ("signal", "mixed", "gnd", "pwr", "jumper")
ROUTED = ("signal", "mixed")

_S, _G, _P = "signal", "gnd", "pwr"
DEFAULTS_BY_COUNT = {
    2: (_S, _S),
    4: (_S, _G, _P, _S),
    6: (_S, _G, _P, _S, _G, _S),
    8: (_S, _G, _S, _P, _G, _S, _G, _S),
}


def is_routed(role: str) -> bool:
    return role in ROUTED


def resolve_layer_roles_ex(copper: list[tuple[str, str]], cfg_roles: dict[str, str]):
    """Return (roles, sources, findings); sources maps layer -> "config" | "kicad" | "default".
    Unresolved layers are absent from both dicts (together with a LAYER_ROLES_NEEDED finding)."""
    use_kicad = any(t != "signal" for _, t in copper)
    default = DEFAULTS_BY_COUNT.get(len(copper))
    roles: dict[str, str] = {}
    sources: dict[str, str] = {}
    missing = []
    for i, (name, ktype) in enumerate(copper):
        if name in cfg_roles:
            roles[name], sources[name] = cfg_roles[name], "config"
        elif use_kicad:
            roles[name], sources[name] = ktype, "kicad"
        elif default is not None:
            roles[name], sources[name] = default[i], "default"
        else:
            missing.append(name)
    findings = []
    if missing:
        findings.append(Finding(
            "LAYER_ROLES_NEEDED", "error",
            f"The board has {len(copper)} copper layers; topor has no default stackup for that count "
            f"and no role is set for: {', '.join(missing)}.",
            'Set each layer\'s role in topor.toml [layers], e.g. In1.Cu = "gnd" (roles: signal, mixed, gnd, pwr, jumper), '
            "or set the layer types in KiCad → File → Board Setup → Board Editor Layers.",
            tuple(missing)))
    return roles, sources, findings


def resolve_layer_roles(copper: list[tuple[str, str]], cfg_roles: dict[str, str]):
    roles, _, findings = resolve_layer_roles_ex(copper, cfg_roles)
    return roles, findings
