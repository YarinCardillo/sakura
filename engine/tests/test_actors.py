"""Actors: who asked for a change, by source."""

import pytest

from sakura import actors


def test_Actors_MasterSource_ParsesAndFormats():
    actor = actors.parse("master:1a2b3c4d")
    assert actor == actors.Actor(actors.MASTER_SOURCE, "1a2b3c4d") and actors.format(actor) == "master:1a2b3c4d"


def test_Actors_MasterSource_IsNotAPersonSource():
    assert actors.MASTER_SOURCE not in actors.PERSON_SOURCES


def test_Actors_MasterWithNode_RoundTrips():
    assert actors.format(actors.parse("master:s1:node-9")) == "master:s1:node-9"


@pytest.mark.parametrize("text", ["master:", "master", "master:a b", "master:a\nb", "Master:abc", "master:a/b"])
def test_Actors_MalformedMasterActor_Refused(text):
    with pytest.raises(actors.ActorError):
        actors.parse(text)


def test_Actors_ResolveFromEnvironment_MasterSource():
    assert actors.resolve(None, {actors.ACTOR_ENV: "master:s1", actors.TRUSTED_ENV: "1"}).source == actors.MASTER_SOURCE


@pytest.mark.parametrize("claimed", ["overlay:owner@example.com:nPHONE", "master:s1"])
def test_Actors_CommandLineClaimsOverlayOrMaster_Refused(claimed):
    with pytest.raises(actors.ActorError, match="cannot claim"):
        actors.resolve(claimed, {})
    with pytest.raises(actors.ActorError, match="cannot claim"):
        actors.resolve(None, {actors.ACTOR_ENV: claimed})


@pytest.mark.parametrize("claimed", ["overlay:owner@example.com:nPHONE", "master:s1"])
def test_Actors_ClaimFromTheCockpitsOwnChild_Accepted(claimed):
    assert actors.format(actors.resolve(claimed, {actors.TRUSTED_ENV: "1"})) == claimed


def test_Actors_TerminalPerson_IsRecordedAsCliUser():
    actor = actors.resolve(None, {})
    assert actor.source == actors.CLI_SOURCE and actor.login
    assert actors.resolve("cli:someone", {}).source == actors.CLI_SOURCE


def test_Actors_AppSource_IsAPersonSourceAndNeedsNoTrust():
    actor = actors.resolve("app:someone", {})
    assert actor.source == actors.APP_SOURCE and actor.source in actors.PERSON_SOURCES
    assert actors.format(actor) == "app:someone"
