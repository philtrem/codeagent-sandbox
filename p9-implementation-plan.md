# 9P2000.L Server Implementation Plan

See `C:\Users\phil\.claude\plans\refactored-chasing-pillow.md` for the full plan.

## Current Step: Step 1 — Wire Format + Message Types

Building the binary serialize/deserialize layer and all 9P2000.L message structs.
This is the foundation everything else builds on.

## Steps Overview
1. Wire format + message types (~25 tests) ← CURRENT
2. FID table (~8 tests)
3. Server loop + session operations (~8 tests)
4. Twalk (~7 tests)
5. Read-only operations (~12 tests)
6. Write operations + WriteInterceptor hooks (~14 tests)
7. Link operations (~5 tests)
8. Robustness (~11 tests)
9. Windows normalization (~7 tests)
10. Sandbox integration (~4 tests)
11. Fuzz target
