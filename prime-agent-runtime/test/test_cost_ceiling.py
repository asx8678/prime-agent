"""Tests for `rlm.set_cost_ceiling` — the tree cost ceiling's kernel API.

The gate itself lives host-side (Rust); these tests pin the kernel API's
contract: the payload shape, the snapshot pass-through, the argument
validation, and that a refused spawn surfaces as the catchable
RuntimeError the docs promise.
"""

from __future__ import annotations

import asyncio
import importlib
import unittest
from typing import Any
from unittest.mock import AsyncMock, patch


rlm = importlib.import_module("rlm")


class SetCostCeilingTest(unittest.TestCase):
    def _call(self, reply: Any, **kwargs: Any) -> tuple[Any, AsyncMock]:
        async def scenario() -> Any:
            host_request = AsyncMock(return_value=reply)
            with patch.object(rlm, "host_request", new=host_request):
                result = await rlm.set_cost_ceiling(**kwargs)
            return result, host_request

        return asyncio.run(scenario())

    def test_sends_the_payload_and_returns_the_snapshot(self) -> None:
        reply = {
            "cost_usd": "0.5",
            "spawns_admitted": 1,
            "ceiling_usd": 2.5,
            "spawn_limit": 4,
        }
        result, host_request = self._call(reply, cost_usd=2.5, spawn_limit=4)
        self.assertEqual(result, reply, "the snapshot passes through verbatim")
        host_request.assert_awaited_once_with(
            "rlm.set_cost_ceiling", {"cost_usd": 2.5, "spawn_limit": 4}
        )

    def test_both_none_clears_the_ceilings(self) -> None:
        reply = {"cost_usd": "0", "spawns_admitted": 0, "ceiling_usd": None, "spawn_limit": None}
        result, host_request = self._call(reply)
        self.assertEqual(result, reply)
        host_request.assert_awaited_once_with(
            "rlm.set_cost_ceiling", {"cost_usd": None, "spawn_limit": None}
        )

    def test_validation_rejects_bad_values(self) -> None:
        for bad_cost in (-1,):
            with self.assertRaises(ValueError):
                self._call({}, cost_usd=bad_cost)
        for bad_cost in ("5", True, [5]):
            with self.assertRaises(TypeError):
                self._call({}, cost_usd=bad_cost)  # type: ignore[arg-type]
        for bad_limit in (0, -3, "4", 2.5, True):
            with self.assertRaises(ValueError):
                self._call({}, spawn_limit=bad_limit)  # type: ignore[arg-type]

    def test_the_namespace_method_delegates(self) -> None:
        async def scenario() -> Any:
            # The host echoes the ceiling it just set in its snapshot reply.
            reply = {"cost_usd": "0", "spawns_admitted": 0, "ceiling_usd": 1.0, "spawn_limit": None}
            host_request = AsyncMock(return_value=reply)
            with patch.object(rlm, "host_request", new=host_request):
                return await rlm.rlm.set_cost_ceiling(cost_usd=1.0), host_request

        result, host_request = asyncio.run(scenario())
        self.assertEqual(result["ceiling_usd"], 1.0)
        host_request.assert_awaited_once_with(
            "rlm.set_cost_ceiling", {"cost_usd": 1.0, "spawn_limit": None}
        )

    def test_a_refused_spawn_raises_the_catchable_runtime_error(self) -> None:
        async def scenario() -> None:
            host_request = AsyncMock(
                side_effect=RuntimeError(
                    "rlm.spawn refused: the session tree's cost ceiling ($1.00) is reached ($1.00 spent)"
                )
            )
            with patch.object(rlm, "host_request", new=host_request):
                await rlm.spawn("child", name="over-budget")

        with self.assertRaises(RuntimeError) as raised:
            asyncio.run(scenario())
        self.assertIn("cost ceiling", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
