"""Eviction-order preference for complete, still-resident request groups.

Never increments references, changes cached hashes, allocates blocks or prevents
vLLM from reclaiming memory. A lease is an eviction preference, not a pin.
"""
from __future__ import annotations
from dataclasses import dataclass
import math


@dataclass(frozen=True)
class ResidentGroup:
    request_id: str
    # All reusable blocks across every KV group, with their generation hashes.
    blocks: dict[int, frozenset[bytes]]
    reusable_tokens: int = 0


def block_hashes(pool, block_id: int):
    """Primary full-block key plus optional partial-prefix aliases."""
    if not 0 <= block_id < len(pool.blocks): return frozenset()
    primary=getattr(pool.blocks[block_id],'block_hash',None)
    return frozenset(pool.cached_block_hashes_by_block.get(block_id,set())) | (frozenset([primary]) if primary is not None else frozenset())


def prioritize(pool, groups: dict[str, ResidentGroup], hints: list[dict], now: float,
               max_fraction: float = .25) -> list[dict]:
    acknowledgements = []
    selected: set[int] = set()
    capacity = int((pool.num_gpu_blocks - 1) * max_fraction)
    ranked = []
    for hint in hints[:3]:
        try:
            score, expiry = float(hint["score"]), float(hint["expires_at"])
            if not math.isfinite(score) or score <= 0 or not now < expiry <= now + 60:
                continue
            ranked.append((score, hint))
        except (ValueError, TypeError, KeyError):
            continue
    # Select highest-value whole groups within the fixed memory quota.
    for score, hint in sorted(ranked, key=lambda item: item[0], reverse=True):
        group = groups.get(hint["request_id"])
        reason = "not_resident"
        if group and group.blocks:
            valid = all(
                0 <= block_id < len(pool.blocks)
                and not pool.blocks[block_id].is_null
                and pool.blocks[block_id].ref_cnt == 0
                and hashes.issubset(block_hashes(pool, block_id))
                and pool.blocks[block_id].prev_free_block is not None
                and pool.blocks[block_id].next_free_block is not None
                for block_id, hashes in group.blocks.items()
            )
            if not valid:
                reason = "active_or_reclaimed"
            elif len(selected | group.blocks.keys()) > capacity:
                reason = "quota"
            else:
                selected.update(group.blocks)
                reason = "prioritized"
        acknowledgements.append({"request_id": hint["request_id"], "status": reason,
                                 "blocks": len(group.blocks) if group else 0,
                                 "expires_at": hint["expires_at"]})
    # Lower-value candidates first, highest value last; shared blocks move once.
    moved = set()
    for _, hint in sorted(ranked, key=lambda item: item[0]):
        group = groups.get(hint["request_id"])
        if group and any(a["request_id"] == group.request_id and a["status"] == "prioritized"
                         for a in acknowledgements):
            for block_id in reversed(list(group.blocks)):
                if block_id in moved:
                    continue
                block = pool.blocks[block_id]
                pool.free_block_queue.remove(block)
                pool.free_block_queue.append(block)
                moved.add(block_id)
    return acknowledgements
