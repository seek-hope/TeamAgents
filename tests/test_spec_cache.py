"""load_team_spec caches validated specs; revisions are immutable so the cache
must never serve stale or wrong-revision data."""
import pytest
from conftest import leader, member, spec_of

from teamagents.storage import Store


def test_load_team_spec_cache_stays_correct_across_revisions(tmp_path):
    store = Store(tmp_path / "s.db")
    store.create_session("s1", str(tmp_path), "approved_scope")
    rev1 = store.save_team_spec("s1", spec_of(leader()))
    rev2 = store.save_team_spec("s1", spec_of(leader(), member("b")))

    latest = store.load_team_spec("s1")
    assert [a.id for a in latest.agents] == ["leader", "b"]
    assert store.load_team_spec("s1") is latest, "second load hits the cache"
    assert [a.id for a in store.load_team_spec("s1", revision=rev1).agents] == ["leader"], \
        "older revisions stay loadable"
    assert rev2 > rev1
    with pytest.raises(KeyError):
        store.load_team_spec("s1", revision=rev2 + 1)
