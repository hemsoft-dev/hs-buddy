"""Receipt clocks only: no event delivery or causality is inferred."""
import time


def stamp():
    return dict(wallMillis=time.time_ns() // 1_000_000,
                monotonicNanos=time.monotonic_ns())
