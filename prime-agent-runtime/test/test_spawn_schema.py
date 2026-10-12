"""Tests for `rlm.spawn(schema=...)` — JSON Schema validation of child results.

The schema is parent-kernel state: `spawn` registers it, `collect` applies
it to the child's settled answer with `jsonschema` (kernel-venv ships 4.26).
These tests mock the host requests, so only the kernel-side halves run —
exactly the design: the schema never crosses to the host.
"""

from __future__ import annotations

import asyncio
import importlib
import unittest
from typing import Any
from unittest.mock import AsyncMock, patch


rlm = importlib.import_module("rlm")


def _spawn_payload(child_id: str = "child-schema-1", name: str = "validator") -> dict[str, Any]:
    return {
        "rlm_child_id": child_id,
        "name": name,
        "session_dir": "/tmp/sessions",
        "model": "provider/model",
    }


def _collect_entry(
    answer_text: str | None,
    *,
    child_id: str = "child-schema-1",
    settled: bool = True,
    status: str = "done",
) -> dict[str, Any]:
    return {
        "rlm_child_id": child_id,
        "session_name": "validator",
        "session_dir": "/tmp/sessions",
        "status": status,
        "settled": settled,
        "answer_preview": None,
        "answer_text": answer_text,
        "error": None,
        "duration_ms": 5,
        "tool_use_count": 0,
        "replied_since_task": False,
    }


# A TypeBox `Type.Object({answer: Type.String()}, {additionalProperties: false})`
# emits exactly this shape at runtime — the carry-over claim the docs make.
_TYPEBOX_SHAPED_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {"answer": {"type": "string"}},
    "required": ["answer"],
    "additionalProperties": False,
}


class SpawnSchemaTest(unittest.TestCase):
    def setUp(self) -> None:
        rlm._spawn_schemas.clear()

    def tearDown(self) -> None:
        rlm._spawn_schemas.clear()

    def _run(self, schema: dict[str, Any] | None, answer_text: str | None, *, collect_targets: Any = None) -> Any:
        """Spawn (registering `schema`) then collect, both over mocked host requests."""

        async def scenario() -> Any:
            host_request = AsyncMock(
                side_effect=[
                    _spawn_payload(),
                    {"results": [_collect_entry(answer_text)]},
                ]
            )
            with patch.object(rlm, "host_request", new=host_request):
                handle = await rlm.spawn(
                    "find the answer",
                    name="validator",
                    schema=schema,
                )
                results = await rlm.collect(collect_targets or handle)
            return handle, results, host_request

        return asyncio.run(scenario())

    def test_valid_result_passes_a_typebox_shaped_schema(self) -> None:
        _, results, _ = self._run(_TYPEBOX_SHAPED_SCHEMA, '{"answer": "yes"}')
        (result,) = results
        self.assertTrue(result.schema_valid)
        self.assertIsNone(result.schema_errors)

    def test_invalid_result_reports_the_first_five_errors(self) -> None:
        # Six required keys, all missing: exactly five errors surface.
        schema = {"type": "object", "required": [f"key{i}" for i in range(6)]}
        _, results, _ = self._run(schema, "{}")
        (result,) = results
        self.assertFalse(result.schema_valid)
        self.assertEqual(len(result.schema_errors or []), 5)
        for error in result.schema_errors or []:
            self.assertIn(":", error)

    def test_answer_without_json_reports_no_json(self) -> None:
        _, results, _ = self._run(_TYPEBOX_SHAPED_SCHEMA, "I could not decide.")
        (result,) = results
        self.assertFalse(result.schema_valid)
        self.assertEqual(
            result.schema_errors,
            ["the child's answer contains no JSON to validate against the schema"],
        )

    def test_fenced_json_block_is_extracted_from_prose(self) -> None:
        answer = 'Sure! Here it is:\n\n```json\n{"answer": "nested"}\n```\n\nLet me know.'
        _, results, _ = self._run(_TYPEBOX_SHAPED_SCHEMA, answer)
        (result,) = results
        self.assertTrue(result.schema_valid)
        self.assertIsNone(result.schema_errors)

    def test_last_fenced_block_wins(self) -> None:
        answer = '```json\n{"answer": "draft"}\n``` thinking ```json\n{"answer": "final"}\n```'
        _, results, _ = self._run(_TYPEBOX_SHAPED_SCHEMA, answer)
        (result,) = results
        self.assertTrue(result.schema_valid)

    def test_bare_trailing_json_object_is_extracted(self) -> None:
        answer = 'The result is {"answer": "bare"} as requested.'
        _, results, _ = self._run(_TYPEBOX_SHAPED_SCHEMA, answer)
        (result,) = results
        self.assertTrue(result.schema_valid)
        self.assertIsNone(result.schema_errors)

    def test_nested_object_in_prose_extracts_the_outer_span(self) -> None:
        # A balanced outer span swallows its nested spans: only the
        # top-level span is a json.loads candidate.
        schema = {
            "type": "object",
            "properties": {"outer": {"type": "object"}},
            "required": ["outer"],
        }
        answer = 'The result: {"outer": {"inner": 1}} as stated.'
        _, results, _ = self._run(schema, answer)
        (result,) = results
        self.assertTrue(result.schema_valid)
        self.assertIsNone(result.schema_errors)

    def test_many_stray_openers_stay_linear_and_the_trailing_object_wins(self) -> None:
        # 20,000 stray `{` used to restart the scan per opener (quadratic);
        # the single-pass stack must dig the trailing object out quickly.
        answer = "{" * 20_000 + '{"answer": "found"}'
        _, results, _ = self._run(_TYPEBOX_SHAPED_SCHEMA, answer)
        (result,) = results
        self.assertTrue(result.schema_valid)
        self.assertIsNone(result.schema_errors)

    def test_truncated_answer_reports_the_host_cap(self) -> None:
        # The host truncates answer_text at ANSWER_TEXT_MAX_CHARS
        # (rlm_child_model.rs): an answer that long with no extractable
        # JSON reports the truncation, not "contains no JSON".
        answer = "no JSON here, just prose. " * 3_500
        self.assertGreaterEqual(len(answer), rlm._ANSWER_TEXT_MAX_CHARS)
        _, results, _ = self._run(_TYPEBOX_SHAPED_SCHEMA, answer)
        (result,) = results
        self.assertFalse(result.schema_valid)
        self.assertEqual(
            result.schema_errors,
            [
                "the child's answer was truncated at the host's "
                f"{rlm._ANSWER_TEXT_MAX_CHARS:,}-character cap, so the JSON could not be parsed"
            ],
        )

    def test_unvalidated_spawn_leaves_the_fields_unset(self) -> None:
        _, results, _ = self._run(None, '{"answer": "yes"}')
        (result,) = results
        self.assertIsNone(result.schema_valid)
        self.assertIsNone(result.schema_errors)

    def test_non_dict_schema_is_a_type_error_at_spawn(self) -> None:
        with self.assertRaises(TypeError):
            asyncio.run(rlm.spawn("x", name="validator", schema=["not", "a", "dict"]))  # type: ignore[arg-type]

    def test_invalid_schema_is_reported_not_raised(self) -> None:
        # `type: 17` is not a valid JSON Schema type: the validator
        # rejects the schema itself, and collect reports it instead of
        # raising. The registry is seeded directly (spawn validates up
        # front, so only a registry that skipped that guard reaches
        # here — the collect-time defense in depth).
        rlm._spawn_schemas["child-schema-1"] = {"type": 17, "required": ["answer"]}

        async def scenario() -> Any:
            host_request = AsyncMock(side_effect=[{"results": [_collect_entry("{}")]}])
            with patch.object(rlm, "host_request", new=host_request):
                return await rlm.collect(timeout_ms=0)

        (result,) = asyncio.run(scenario())
        self.assertFalse(result.schema_valid)
        self.assertTrue((result.schema_errors or [""])[0].startswith("invalid schema:"))

    def test_spawn_with_an_invalid_schema_raises_before_the_child_starts(self) -> None:
        # The schema is validated at spawn time, before `rlm.run` reaches
        # the host: no child is started for a schema jsonschema rejects.
        host_request = AsyncMock()
        with patch.object(rlm, "host_request", new=host_request):
            with self.assertRaises(ValueError) as ctx:
                asyncio.run(rlm.spawn("x", name="validator", schema={"type": 17}))
        self.assertIn("schema is not a valid JSON Schema", str(ctx.exception))
        host_request.assert_not_called()
        self.assertEqual(rlm._spawn_schemas, {})

    def test_collect_by_name_validates_through_the_registry(self) -> None:
        # The handle is one selector form among three: a name string
        # resolves the same registered schema.
        _, results, _ = self._run(
            _TYPEBOX_SHAPED_SCHEMA,
            '{"answer": "named"}',
            collect_targets="validator",
        )
        (result,) = results
        self.assertTrue(result.schema_valid)

    def test_unsettled_results_skip_validation(self) -> None:
        _, results, _ = self._run(
            _TYPEBOX_SHAPED_SCHEMA,
            "not json yet",
        )
        # Override the entry to unsettled: rerun with settled=False.
        rlm._spawn_schemas.clear()

        async def scenario() -> Any:
            host_request = AsyncMock(
                side_effect=[
                    _spawn_payload(),
                    {"results": [_collect_entry("running...", settled=False, status="running")]},
                ]
            )
            with patch.object(rlm, "host_request", new=host_request):
                await rlm.spawn("x", name="validator", schema=_TYPEBOX_SHAPED_SCHEMA)
                return await rlm.collect(timeout_ms=0)

        (result,) = asyncio.run(scenario())
        self.assertIsNone(result.schema_valid)
        self.assertIsNone(result.schema_errors)

    def test_handle_carries_the_schema(self) -> None:
        handle, _, _ = self._run(_TYPEBOX_SHAPED_SCHEMA, '{"answer": "yes"}')
        self.assertIs(handle.schema, _TYPEBOX_SHAPED_SCHEMA)

    def test_spawn_never_sends_the_schema_to_the_host(self) -> None:
        _, _, host_request = self._run(_TYPEBOX_SHAPED_SCHEMA, '{"answer": "yes"}')
        spawn_call = host_request.call_args_list[0]
        self.assertEqual(spawn_call.args[0], "rlm.run")
        self.assertNotIn("schema", str(spawn_call.args[1]["kwargs"]))

    def test_the_rlm_namespace_forwards_the_schema(self) -> None:
        # Cells see `rlm` as the namespace object (the Rust bootstrap binds
        # `rlm = rlm_module.rlm`), so its spawn must forward schema= too.
        async def scenario() -> Any:
            host_request = AsyncMock(side_effect=[_spawn_payload()])
            with patch.object(rlm, "host_request", new=host_request):
                return await rlm.rlm.spawn("x", name="validator", schema=_TYPEBOX_SHAPED_SCHEMA)

        handle = asyncio.run(scenario())
        self.assertIs(handle.schema, _TYPEBOX_SHAPED_SCHEMA)
        self.assertIs(rlm._spawn_schemas[handle.rlm_child_id], _TYPEBOX_SHAPED_SCHEMA)


if __name__ == "__main__":
    unittest.main()
