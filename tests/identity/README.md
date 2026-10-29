# Disposable credential-service fixtures

Run these scripts only in the isolated test containers/private D-Bus sessions.
They initialize ephemeral OS-store credentials; never use a personal store.

On a Linux native-store test or signed-pilot fixture failure, `credential_probe.py`
reports only fixed metadata fields: whether the private fixture scope is valid,
service presence, default-collection lock state, and whether the service owner
changed while probing. Missing/failed information stays unknown, not unlocked.
The report does not identify items or include values, fingerprints, account names,
owner IDs, paths, passwords or raw backend errors.

The probe requires the private-fixture marker and Unix-only session-bus addresses.
It checks name ownership on the bus daemon first, then targets only the existing
unique owner. It cannot activate a missing/replacement credential service. It
reads only alias/lock metadata, never credentials, and never unlocks or retries
an operation. Each D-Bus call has a 2-second timeout and its subprocess a 3-second
bound; only responses up to 4 KiB are accepted. Metadata is a best-effort snapshot,
not a diagnosis of the original failure. The original exception/exit code remains.
The helper and `gdbus` are included in fixture images only.

Pure probe tests need no bus or credentials:

```sh
python3 -m unittest discover -s tests/identity -p 'test_credential_probe.py' -v
```

An intermittent unavailable/locked-store failure previously seen in CI has not
been reproduced by 100 parallel private-store reads. This diagnostic does not
claim to fix that failure or justify automatic CI retries.
