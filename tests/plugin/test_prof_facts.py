"""TW0: the plugin completes the engine profile's python split (result.json `profile`)."""
from topor.run import RunOutcome, _profile_facts


def test_plugin_completes_the_python_split():
    out = RunOutcome("accepted", wall_s=130.0, profile={"stages": [], "python": {"before_s": 10.0, "job_s": 100.0}})
    py = _profile_facts(out)["python"]
    assert py["after_s"] == 20.0 and py["total_s"] == 130.0
    assert _profile_facts(RunOutcome("accepted")) is None
