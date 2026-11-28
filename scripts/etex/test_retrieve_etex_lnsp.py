"""Test how the ETEX replay handles CDS responses without network access."""

from contextlib import redirect_stderr
from io import StringIO
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import retrieve_etex_lnsp as retrieval


MODEL = "era5-19941023-151821-ml.grib"
PRESSURE = "era5-19941023-151821-lnsp.grib"
OFFICIAL_CDS_API = "https://cds.climate.copernicus.eu/api"


class ReplayTests(unittest.TestCase):
    def replay(self, directory, filename, response):
        client = Mock()
        client.retrieve.side_effect = response
        factory = Mock(return_value=client)
        argv = [retrieval.__file__, "23", str(directory)]
        if filename == MODEL:
            argv.append("--replay-model")
        with (
            patch.dict(sys.modules, {"cdsapi": SimpleNamespace(Client=factory)}),
            patch.dict(os.environ, {"CDSAPI_URL": "https://example.invalid"}, clear=True),
            patch.object(sys, "argv", argv),
        ):
            retrieval.main()
        return factory, client

    def test_replays_both_pinned_responses_through_official_cds(self):
        for filename, manifest in (
            (MODEL, "request.json"),
            (PRESSURE, "era5-19941023-151821-lnsp.request.json"),
        ):
            with self.subTest(filename=filename), TemporaryDirectory() as scratch:
                directory = Path(scratch)
                raw = (retrieval.SOURCE_ROOT / filename).read_bytes()
                pinned = json.loads((retrieval.SOURCE_ROOT / manifest).read_bytes())
                factory, client = self.replay(
                    directory, filename, lambda _dataset, _request, path: Path(path).write_bytes(raw)
                )
                self.assertEqual(factory.call_args.kwargs["url"], OFFICIAL_CDS_API)
                self.assertIs(factory.call_args.kwargs["verify"], True)
                self.assertEqual(
                    client.retrieve.call_args.args[:2], (pinned["dataset"], pinned["request"])
                )
                self.assertEqual((directory / filename).read_bytes(), raw)
                record = json.loads((directory / filename).with_suffix(".request.json").read_bytes())
                self.assertEqual(record["sha256"], pinned["sha256"])
                self.assertEqual(record["api_url"], OFFICIAL_CDS_API)

    def test_changed_or_truncated_grib_never_publishes_a_receipt(self):
        raw = (retrieval.SOURCE_ROOT / PRESSURE).read_bytes()
        for changed in (raw[:-1] + bytes([raw[-1] ^ 1]), raw[:-1]):
            with TemporaryDirectory() as scratch, redirect_stderr(StringIO()) as stderr:
                directory = Path(scratch)
                with self.assertRaises(SystemExit) as exit_code:
                    self.replay(directory, PRESSURE, lambda _d, _r, path: Path(path).write_bytes(changed))
                self.assertEqual(exit_code.exception.code, 1)
                self.assertIn("does not match the pinned", stderr.getvalue())
                self.assertEqual(list(directory.iterdir()), [directory / (PRESSURE + ".part")])

    def test_provider_failure_has_separate_sanitized_error(self):
        with TemporaryDirectory() as scratch, redirect_stderr(StringIO()) as stderr:
            directory = Path(scratch)

            def deny(*_args):
                error = RuntimeError("credential in provider error")
                error.response = SimpleNamespace(status_code=403)
                raise error

            with self.assertRaises(SystemExit) as exit_code:
                self.replay(directory, PRESSURE, deny)
            self.assertEqual(exit_code.exception.code, 1)
            self.assertIn("CDS retrieval failed (RuntimeError, HTTP 403)", stderr.getvalue())
            self.assertNotIn("credential in provider error", stderr.getvalue())
            self.assertEqual(list(directory.iterdir()), [directory / (PRESSURE + ".part")])


if __name__ == "__main__":
    unittest.main()
