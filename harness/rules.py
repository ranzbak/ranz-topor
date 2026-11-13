"""Rule resolution (SWIG front end): the board's design settings and effective net classes are read with
pcbnew and handed to the pure resolver `toporlib.rules.resolve_rules_from_project` (the plugin feeds the
same function from IPC + the .kicad_pro). Board Setup constraints, netclasses, custom-rule (.kicad_dru) subset,
widths; the JLCPCB-derived fallback only for a field the board does not provide."""
from __future__ import annotations

from harness.rip import NetWidths
from toporlib.dru import (  # noqa: F401  (re-export)
    _UNITS, DruRule, _match_term, _parse_sexp, _tokenize, _value_nm, match_condition, parse_dru,
)
from toporlib.rules import (  # noqa: F401  (re-export)
    DEFAULT_FAB_PROFILE, JLCPCB_1OZ, RULE_FALLBACK, NetClassVals, _POWER_RE, _REFUSE, resolve_rules_from_project,
)


def _swig_inputs(board) -> tuple[dict, dict, dict, dict]:
    """(pro, classes, netclass_of, net_codes) read from a pcbnew board. `pro` is synthesised from the
    design settings (mm); every effective (composite) class is registered under its own name with the
    values pcbnew reports, so the pure resolver needs no composition logic here."""
    ds = board.GetDesignSettings()
    ns = ds.m_NetSettings
    mm = lambda v: int(v) / 1e6  # noqa: E731
    pro = {"board": {"design_settings": {"rules": {
        "min_clearance": mm(ds.m_MinClearance),
        "min_track_width": mm(ds.m_TrackMinWidth),
        "min_via_diameter": mm(ds.m_ViasMinSize),
        "min_through_hole_diameter": mm(ds.m_MinThroughDrill),
        "min_via_annular_width": mm(ds.m_ViasMinAnnularWidth),
        "min_hole_clearance": mm(ds.m_HoleClearance),
        "min_hole_to_hole": mm(ds.m_HoleToHoleMin),
        "min_copper_edge_clearance": mm(ds.m_CopperEdgeClearance),
        "min_connection": mm(ds.m_MinConn),
        "max_error": mm(ds.m_MaxError),
    }, "via_dimensions": [{"diameter": mm(v.m_Diameter), "drill": mm(v.m_Drill)}
                          for v in ds.m_ViasDimensionsList]}}}
    # Violation severities live in the project file only (no stable SWIG accessor): read them from it. A design
    # rule key the project file does not carry is left out (pcbnew holds its built-in value then), so the resolver
    # sees what the plugin sees in the .kicad_pro and applies the reported fallback (toporlib.rules RULE_FALLBACK).
    import json
    from pathlib import Path

    file_ds: dict = {}
    try:
        pro_p = Path(str(board.GetFileName())).with_suffix(".kicad_pro")
        if pro_p.is_file():
            file_ds = json.loads(pro_p.read_text()).get("board", {}).get("design_settings", {}) or {}
    except (OSError, ValueError):
        file_ds = {}
    file_rules = file_ds.get("rules") or {}
    syn = pro["board"]["design_settings"]["rules"]
    for k in list(syn):
        if k not in file_rules and not (k == "max_error" and k in file_ds):
            del syn[k]
        elif isinstance(file_rules.get(k, file_ds.get(k)), (int, float)):
            # the file's own value: pcbnew has already reset a value outside its load range (e.g. max_error 0),
            # the resolver mirrors that reset and reports it, as the plugin does (toporlib.rules KICAD_RANGE_NM)
            syn[k] = file_rules.get(k, file_ds.get(k))
    if file_ds.get("rule_severities"):
        pro["board"]["design_settings"]["rule_severities"] = file_ds["rule_severities"]
    classes: dict[str, NetClassVals] = {}
    of: dict[str, str] = {}
    codes: dict[str, int] = {}
    names = [""]
    for code, n in sorted(board.GetNetsByNetcode().items()):
        if int(code) != 0:
            codes[str(n.GetNetname())] = int(code)
            names.append(str(n.GetNetname()))
    for name in names:
        nc = ns.GetEffectiveNetClass(name)
        nm = str(nc.GetName())
        of[name] = nm
        classes.setdefault(nm, NetClassVals(nm, int(nc.GetClearance()), int(nc.GetTrackWidth()),
                                            int(nc.GetViaDiameter()), int(nc.GetViaDrill())))
    return pro, classes, of, codes


def resolve_rules(
    board,
    *,
    dru_text: str | None,
    widths_before: dict[str, NetWidths],
    settings_table: dict[str, int] | None = None,
    fab_profile: str = DEFAULT_FAB_PROFILE,
) -> dict:
    """Resolve the design rules from the board's KiCad configuration. See
    `toporlib.rules.resolve_rules_from_project` for the semantics (`fab_profile` = ``[rules] fab_profile``:
    ``"board"`` (default) or the opt-in floor profile ``"jlcpcb_1oz"``; ``fallbacks``, ``floors_applied``,
    warnings)."""
    pro, classes, of, codes = _swig_inputs(board)
    return resolve_rules_from_project(
        pro, dru_text, netclass_of=of, classes=classes, widths_before=widths_before,
        settings_table=settings_table, fab_profile=fab_profile,
        copper_layer_count=int(board.GetCopperLayerCount()), net_codes=codes,
        layer_names={int(l): str(board.GetLayerName(l)) for l in board.GetEnabledLayers().CuStack()})
