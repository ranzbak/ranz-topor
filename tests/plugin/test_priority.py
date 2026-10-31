"""B7: @N markers, topor_pN netclasses, [nets] priority, power pattern (46 §2.1)."""
import pytest

from topor.config import Config
from topor.priority import net_priorities, parse_marker

CFG = Config.defaults()


@pytest.mark.parametrize("name,want", [
    ("USB_D@1+", 1), ("CLK@1_P", 1), ("DQ@1_3", 1), ("DQ@13", None), ("/ddr/DQ@1_0", 1),
    ("~{RESET}@2", 2), ("~{RESET@2}", 2), ("A@4", None), ("@2", 2), ("/a@1/B", None), ("PLAIN", None),
])
def test_marker_parse_table(name, want):
    assert parse_marker(name) == want


def test_marker_ignored_finding():
    pr, fs = net_priorities(["DQ@13", "A@4", "OK@2"], {}, CFG)
    assert pr == {"OK@2": 2}
    ig = [f for f in fs if f.code == "MARKER_IGNORED"]
    assert len(ig) == 1 and set(ig[0].items) == {"DQ@13", "A@4"}
    assert ig[0].fix


def test_priority_sources_and_conflicts():
    cfg = CFG.with_values({("nets", "priority"): {"T": 3, "TC*": 2}})
    nets = ["M@1", "NC", "MC@1", "T", "TC1", "NONE", "ONLY_CLASS"]
    of = {"NC": "topor_p2", "MC@1": "topor_p3", "T": "topor_p1", "ONLY_CLASS": "topor_p3"}
    pr, fs = net_priorities(nets, of, cfg)
    assert pr["M@1"] == 1
    assert pr["NC"] == 2
    assert pr["MC@1"] == 1  # marker wins over the netclass
    assert pr["T"] == 1  # netclass wins over [nets] priority
    assert pr["TC1"] == 2  # glob in [nets] priority
    assert "NONE" not in pr
    assert pr["ONLY_CLASS"] == 3
    conf = {i for f in fs if f.code == "MARKER_CONFLICT" for i in f.items}
    assert conf == {"MC@1", "T"}
    assert all(f.fix for f in fs)


def test_toml_only_when_no_other_source():
    cfg = CFG.with_values({("nets", "priority"): {"X": 2}})
    pr, fs = net_priorities(["X"], {}, cfg)
    assert pr == {"X": 2} and not fs


def test_diff_pair_higher_wins():
    pr, fs = net_priorities(["USB_D@1+", "USB_D@2-", "CLK@3_P", "CLK@3_N", "S@2+", "S@2-"], {}, CFG)
    assert pr["USB_D@1+"] == 1 and pr["USB_D@2-"] == 1
    assert pr["CLK@3_P"] == 3 and pr["CLK@3_N"] == 3
    pair = [f for f in fs if f.code == "MARKER_PAIR"]
    assert len(pair) == 1 and set(pair[0].items) == {"USB_D@1+", "USB_D@2-"}


def test_diff_pair_unmarked_half_follows():
    pr, fs = net_priorities(["Q@2+", "Q@2-"], {"Q@2-": "Default"}, CFG)
    assert pr == {"Q@2+": 2, "Q@2-": 2} and not fs
    pr, fs = net_priorities(["R+", "R-"], {"R+": "topor_p1"}, CFG)
    assert pr == {"R+": 1, "R-": 1} and not fs


def test_power_by_netclass_name_and_pattern():
    of = {"V12": "MyPower", "+5V": "Default", "SIG": "Default", "HS@1": "Default", "+3V3": "topor_p1"}
    pr, fs = net_priorities(["V12", "+5V", "SIG", "GND", "+3V3", "HS@1", "VCC_X"], of, CFG)
    assert pr["V12"] == 2
    assert pr["+5V"] == 2 and pr["GND"] == 2 and pr["VCC_X"] == 2  # missing class == Default
    assert pr["+3V3"] == 1  # set explicitly, power does not override
    assert pr["HS@1"] == 1
    assert "SIG" not in pr
    pw = [f for f in fs if f.code == "POWER_BY_NAME"]
    assert len(pw) == 1 and set(pw[0].items) == {"+5V", "GND", "VCC_X"}
    assert pw[0].severity == "info" and pw[0].fix


def test_power_pattern_only_for_default_netclass():
    of = {"+5V": "HighSpeed", "GND": "Default"}
    pr, fs = net_priorities(["+5V", "GND"], of, CFG)
    assert "+5V" not in pr  # in a non-Default class: the pattern does not apply
    assert pr["GND"] == 2
    pr2, _ = net_priorities(["GND"], {}, CFG.with_values({("nets", "power_pattern"): "^NOPE"}))
    assert pr2 == {}
