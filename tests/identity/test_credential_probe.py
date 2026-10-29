import os
import subprocess
import unittest
from unittest.mock import Mock, patch

from credential_probe import BUS, SERVICE, _call, snapshot


class CredentialProbeTests(unittest.TestCase):
    def setUp(self):
        self.environment = patch.dict(os.environ, {
            "OUR_CLIPBOARD_PRIVATE_KEYRING_BUS": "1",
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/private-test-only",
        }, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)

    @staticmethod
    def fixture(locked="false", alias="/org/freedesktop/secrets/collection/login", changed=False):
        queries = []
        owners = iter(["(':1.2',)", "(':1.3',)" if changed else "(':1.2',)"])
        def call(destination, path, method, *args):
            queries.append((destination, method))
            if method.endswith(".NameHasOwner"):
                return "(true,)"
            if method.endswith(".GetNameOwner"):
                return next(owners)
            if method.endswith(".ReadAlias"):
                return "(objectpath '" + alias + "',)"
            if method.endswith(".Get"):
                return "(<" + locked + ">,)"
            raise AssertionError("Unexpected credential operation")
        return call, queries

    def test_locked_unlocked_missing_and_owner_replacement_are_distinct(self):
        for locked, alias, changed, expected in [
            ("false", "/org/freedesktop/secrets/collection/login", False, "unlocked"),
            ("true", "/org/freedesktop/secrets/collection/login", True, "locked"),
            ("false", "/", False, "missing"),
        ]:
            with self.subTest(expected=expected):
                call, queries = self.fixture(locked, alias, changed)
                self.assertEqual(snapshot(call), {"private_fixture": True, "service": "present",
                                                 "collection": expected, "owner_changed": changed})
                for destination, method in queries:
                    if destination != BUS:
                        self.assertTrue(destination.startswith(":"))
                    self.assertNotIn(method, ["org.freedesktop.Secret.Service.Unlock",
                                             "org.freedesktop.Secret.Item.GetSecret"])
                    self.assertNotEqual(destination, SERVICE)

    def test_missing_service_and_failed_probe_are_not_reported_as_unlocked(self):
        absent = Mock(return_value="(false,)")
        self.assertEqual(snapshot(absent)["service"], "absent")
        absent.assert_called_once()
        for response in [None, "SECRET-DO-NOT-ECHO", "(invalid,)"]:
            result = snapshot(Mock(return_value=response))
            self.assertEqual(result["service"], "unknown")
            self.assertNotIn("SECRET", str(result))
        self.assertEqual(snapshot(Mock(side_effect=OSError()))["service"], "unknown")

    def test_personal_or_nonlocal_bus_is_never_queried(self):
        for variable, value in [("OUR_CLIPBOARD_PRIVATE_KEYRING_BUS", ""),
                                ("DBUS_SESSION_BUS_ADDRESS", "tcp:host=localhost"),
                                ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/private;tcp:host=localhost")]:
            with patch.dict(os.environ, {variable: value}):
                call = Mock(side_effect=AssertionError("Unexpected bus access"))
                self.assertFalse(snapshot(call)["private_fixture"])
                call.assert_not_called()

    def test_subprocess_is_bounded_and_does_not_forward_output_on_failure(self):
        for effect in [OSError(), subprocess.TimeoutExpired("gdbus", 3)]:
            with patch("credential_probe.subprocess.run", side_effect=effect):
                self.assertIsNone(_call(":1.2", "/", "test"))
        for code, stdout in [(1, "SECRET-DO-NOT-ECHO"), (0, "x" * 4097)]:
            with patch("credential_probe.subprocess.run", return_value=Mock(returncode=code, stdout=stdout)) as run:
                self.assertIsNone(_call(":1.2", "/", "test"))
                self.assertEqual(run.call_args.kwargs["timeout"], 3)
                self.assertTrue(run.call_args.kwargs["capture_output"])


if __name__ == "__main__":
    unittest.main()
