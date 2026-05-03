#!/usr/bin/env python3
"""Post one Polymarket CLOB V2 order from a JSON request on stdin."""

from __future__ import annotations

import json
import os
import sys
from typing import Any


def env_required(name: str) -> str:
    value = os.getenv(name, "").strip()
    if not value:
        raise RuntimeError(f"{name} is required for live trading")
    return value


def optional_int(name: str) -> int | None:
    value = os.getenv(name, "").strip()
    return int(value) if value else None


def extract_order_id(response: Any) -> str | None:
    if isinstance(response, dict):
        for key in ("orderID", "orderId", "order_id", "id"):
            if response.get(key):
                return str(response[key])
    return None


def main() -> int:
    try:
        from py_clob_client_v2 import (
            ApiCreds,
            ClobClient,
            OrderArgs,
            OrderType,
            PartialCreateOrderOptions,
            Side,
        )
    except ImportError as exc:
        raise RuntimeError("install py-clob-client-v2 first: pip install py-clob-client-v2") from exc

    request = json.load(sys.stdin)
    side = Side.BUY if request["side"].upper() == "BUY" else Side.SELL
    order_type = getattr(OrderType, request.get("order_type") or os.getenv("LIVE_ORDER_TYPE", "GTC"))

    creds = ApiCreds(
        api_key=env_required("POLYMARKET_API_KEY"),
        api_secret=env_required("POLYMARKET_API_SECRET"),
        api_passphrase=env_required("POLYMARKET_API_PASSPHRASE"),
    )

    kwargs: dict[str, Any] = {
        "host": os.getenv("POLYMARKET_CLOB_HOST", "https://clob.polymarket.com"),
        "chain_id": int(os.getenv("POLYMARKET_CHAIN_ID", "137")),
        "key": env_required("POLYMARKET_PRIVATE_KEY"),
        "creds": creds,
    }
    signature_type = optional_int("SIGNATURE_TYPE")
    if signature_type is not None:
        kwargs["signature_type"] = signature_type
    funder = os.getenv("FUNDER_ADDRESS", "").strip()
    if funder:
        kwargs["funder"] = funder

    client = ClobClient(**kwargs)
    response = client.create_and_post_order(
        order_args=OrderArgs(
            token_id=str(request["token_id"]),
            price=float(request["price"]),
            side=side,
            size=float(request["size"]),
        ),
        options=PartialCreateOrderOptions(tick_size=os.getenv("POLYMARKET_TICK_SIZE", "0.01")),
        order_type=order_type,
    )

    print(
        json.dumps(
            {
                "success": bool(response.get("success", True)) if isinstance(response, dict) else True,
                "order_id": extract_order_id(response),
                "raw": response,
            },
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        raise SystemExit(1)
