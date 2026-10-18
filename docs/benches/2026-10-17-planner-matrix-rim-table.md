# Planner matrix, before and after (2026-10-17): the shipped planner vs branch planner/r2 (R2 + the thin family), 28 routes, warm, 100 s, seconds judge

Rows from docs/benches/2026-10-17-planner-matrix-{before,thin5,before3,thin5-3}.jsonl via knobs/planner_before_after.py --judge seconds. Columns: route, straight ly, jumps before / after, stops, pilot seconds, first route s, wall s, verdict. The external reference (--ref) rows sit beside for the eye.

## PC, 24 threads: before (main 4b79562) -> thin5

| route | straight ly | jumps b/a | stops b/a | pilot s b/a | first s b/a | wall s b/a | ref jumps / s | verdict |
|---|---|---|---|---|---|---|---|---|
| Beagle Point -> Sol | 65279 | 197 / 197 | 9 / 9 | 14597 / 14592 | 0.1 / 0.2 | 1.5 / 0.6 | 198 / 29 | better |
| Beagle Point -> Wongi | 65182 | 196 / 196 | 10 / 10 | 14555 / 14518 | 0.3 / 0.2 | 1.6 / 0.6 | 198 / 28 | better |
| Blaa Hypai AA-A a96-19 -> Wongi | 12712 | 36 / 36 | 1 / 1 | 2674 / 2674 | 0.1 / 0.1 | 0.2 / 0.3 | 37 / 10 | same |
| Byoi Fraae AT-U d2-1 -> Jongou AA-A d0 | - | - / - | - / - | - / - | 0.0 / 0.0 | 100.2 / 80.0 | 477 / 61 | same |
| Byoi Fraae CQ-G d10-0 -> Jongou XM-W d1-0 | - | - / - | - / - | - / - | 0.0 / 0.0 | 100.2 / 82.8 | 516 / 61 | same |
| Byoomao AA-A c1 -> Sol | 24621 | 62 / 62 | 3 / 3 | 4708 / 4694 | 0.1 / 0.1 | 0.6 / 0.8 | 61 / 2 | better |
| Colonia -> Sol | 22000 | 55 / 55 | 2 / 2 | 4170 / 4161 | 0.2 / 0.1 | 0.9 / 1.0 | 54 / 15 | better |
| Colonia -> Spase AA-A a108-0 | 31440 | 91 / 88 | 4 / 4 | 6746 / 6582 | 1.4 / 1.1 | 52.4 / 5.2 (same-day) | 90 / 61 | better |
| Colonia -> Wongi | 21939 | 54 / 54 | 2 / 2 | 4096 / 4096 | 0.4 / 0.2 | 1.2 / 0.9 | 54 / 16 | same |
| Jongou AA-A d0 -> Byoi Fraae AT-U d2-1 | 85526 | 396 / 396 | 17 / 17 | 27520 / 27517 | 1.3 / 1.6 | 100.2 / 100.2 | 492 / 62 | better |
| Jongou XM-W d1-0 -> Byoi Fraae CQ-G d10-0 | 87012 | 402 / 401 | 17 / 17 | 28028 / 27974 | 100.0 / 14.3 | 108.9 / 55.0 | 530 / 61 | better |
| Ood Fleau ZJ-I d9-0 -> Sphiesi HX-L d7-0 | - | - / 412 | - / 17 | - / 28490 | 0.0 / 24.8 | 100.3 / 101.9 | 451 / 61 | better |
| Pheia Auscs AA-A d0 -> Spase AA-A a108-0 | 13439 | 37 / 37 | 1 / 1 | 2760 / 2760 | 0.1 / 0.1 | 0.4 / 0.7 | 38 / 1 | same |
| Plielou RN-R d5-27 -> Wongi | 12117 | 36 / 37 | 1 / 1 | 2648 / 2643 | 0.1 / 0.1 | 0.2 / 0.4 | 36 / 3 | better |
| Sagittarius A* -> Sol | 25900 | 73 / 73 | 3 / 3 | 5428 / 5434 | 2.0 / 0.9 | 6.3 / 2.7 | 70 / 38 | better |
| Sol -> Beagle Point | 65279 | 191 / 191 | 10 / 10 | 14273 / 14273 | 0.3 / 0.2 | 77.4 / 76.7 (same-day) | 205 / 61 | same |
| Sol -> Byoomao AA-A c1 | 24621 | 64 / 64 | 3 / 3 | 4827 / 4871 | 0.2 / 0.1 | 0.6 / 0.9 | 74 / 61 | WORSE |
| Sol -> Colonia | 22000 | 58 / 58 | 3 / 3 | 4360 / 4358 | 0.1 / 0.2 | 1.0 / 1.2 | 60 / 38 | better |
| Sol -> Sagittarius A* | 25900 | 70 / 68 | 3 / 4 | 5283 / 5146 | 0.1 / 0.2 | 0.8 / 1.0 | 78 / 60 | WORSE |
| Spase AA-A a108-0 -> Colonia | 31440 | 87 / 87 | 5 / 5 | 6516 / 6516 | 2.0 / 1.2 | 7.2 / 6.2 | 85 / 61 | same |
| Spase AA-A a108-0 -> Pheia Auscs AA-A d0 | 13439 | 36 / 36 | 1 / 1 | 2695 / 2694 | 0.0 / 0.1 | 0.3 / 0.6 | 38 / 2 | better |
| Spase AA-A a108-0 -> Wongi | 38200 | 102 / 102 | 5 / 5 | 7790 / 7790 | 0.1 / 0.1 | 0.4 / 0.5 | 96 / 4 | same |
| Sphiesi HX-L d7-0 -> Ood Fleau ZJ-I d9-0 | - | - / 448 | - / 19 | - / 30514 | 0.0 / 26.5 | 100.2 / 107.5 | 591 / 61 | better |
| Wongi -> Beagle Point | 65182 | 191 / 191 | 10 / 10 | 14275 / 14274 | 0.3 / 0.2 | 80.3 / 77.1 (same-day) | 213 / 61 | better |
| Wongi -> Blaa Hypai AA-A a96-19 | 12712 | 38 / 38 | 1 / 1 | 2792 / 2792 | 0.2 / 0.2 | 1.0 / 1.2 | 52 / 61 | same |
| Wongi -> Colonia | 21939 | 58 / 58 | 2 / 2 | 4351 / 4351 | 0.1 / 0.2 | 1.4 / 1.4 | 59 / 61 | same |
| Wongi -> Plielou RN-R d5-27 | 12117 | 37 / 37 | 2 / 2 | 2751 / 2749 | 0.1 / 0.1 | 0.6 / 0.8 | 36 / 47 | better |
| Wongi -> Spase AA-A a108-0 | 38200 | 101 / 101 | 6 / 6 | 7726 / 7726 | 0.1 / 0.1 | 0.6 / 0.8 | 104 / 61 | same |

better 15 · same 11 · WORSE 2 of 28

## Three threads (the box's long lane): before3 -> thin5-3

| route | straight ly | jumps b/a | stops b/a | pilot s b/a | first s b/a | wall s b/a | ref jumps / s | verdict |
|---|---|---|---|---|---|---|---|---|
| Beagle Point -> Sol | 65279 | 197 / 197 | 9 / 9 | 14597 / 14592 | 0.1 / 0.1 | 1.7 / 0.8 | 198 / 29 | better |
| Beagle Point -> Wongi | 65182 | 196 / 196 | 10 / 10 | 14555 / 14518 | 0.1 / 0.1 | 1.3 / 0.9 | 198 / 28 | better |
| Blaa Hypai AA-A a96-19 -> Wongi | 12712 | 36 / 36 | 1 / 1 | 2674 / 2674 | 0.0 / 0.1 | 0.2 / 0.4 | 37 / 10 | same |
| Byoi Fraae AT-U d2-1 -> Jongou AA-A d0 | - | - / - | - / - | - / - | 0.0 / 0.0 | 100.1 / 101.1 | 477 / 61 | same |
| Byoi Fraae CQ-G d10-0 -> Jongou XM-W d1-0 | - | - / - | - / - | - / - | 0.0 / 0.0 | 100.2 / 101.1 | 516 / 61 | same |
| Byoomao AA-A c1 -> Sol | 24621 | 62 / 62 | 3 / 3 | 4708 / 4714 | 0.0 / 0.1 | 0.6 / 0.7 | 61 / 2 | same |
| Colonia -> Sol | 22000 | 55 / 56 | 2 / 2 | 4161 / 4248 | 0.1 / 0.1 | 1.0 / 1.0 | 54 / 15 | WORSE |
| Colonia -> Spase AA-A a108-0 | 31440 | 91 / 88 | 4 / 4 | 6746 / 6582 | 0.8 / 5.9 | 37.0 / 13.6 | 90 / 61 | better |
| Colonia -> Wongi | 21939 | 54 / 55 | 2 / 3 | 4096 / 4201 | 0.1 / 0.1 | 1.1 / 1.1 | 54 / 16 | WORSE |
| Jongou AA-A d0 -> Byoi Fraae AT-U d2-1 | 85526 | 396 / 396 | 17 / 17 | 27520 / 27517 | 1.0 / 1.1 | 22.8 / 52.7 | 492 / 62 | better |
| Jongou XM-W d1-0 -> Byoi Fraae CQ-G d10-0 | - | - / 401 | - / 17 | - / 27974 | 0.0 / 9.8 | 100.1 / 100.5 | 530 / 61 | better |
| Ood Fleau ZJ-I d9-0 -> Sphiesi HX-L d7-0 | - | - / 412 | - / 17 | - / 28490 | 0.0 / 6.4 | 100.1 / 102.6 | 451 / 61 | better |
| Pheia Auscs AA-A d0 -> Spase AA-A a108-0 | 13439 | 37 / 37 | 1 / 1 | 2760 / 2760 | 0.0 / 0.1 | 0.4 / 0.7 | 38 / 1 | same |
| Plielou RN-R d5-27 -> Wongi | 12117 | 36 / 37 | 1 / 1 | 2648 / 2643 | 0.0 / 0.1 | 0.3 / 0.5 | 36 / 3 | better |
| Sagittarius A* -> Sol | 25900 | 72 / 73 | 4 / 3 | 5414 / 5427 | 0.9 / 1.9 | 5.3 / 6.6 | 70 / 38 | WORSE |
| Sol -> Beagle Point | 65279 | 191 / 191 | 10 / 10 | 14273 / 14273 | 0.1 / 0.2 | 50.3 / 100.4 | 205 / 61 | WORSE |
| Sol -> Byoomao AA-A c1 | 24621 | 64 / 64 | 3 / 3 | 4827 / 4871 | 0.0 / 0.1 | 0.6 / 0.9 | 74 / 61 | WORSE |
| Sol -> Colonia | 22000 | 58 / 58 | 3 / 3 | 4360 / 4358 | 0.2 / 0.1 | 1.1 / 1.3 | 60 / 38 | better |
| Sol -> Sagittarius A* | 25900 | 68 / 68 | 4 / 4 | 5140 / 5146 | 0.1 / 0.1 | 1.3 / 1.0 | 78 / 60 | same |
| Spase AA-A a108-0 -> Colonia | 31440 | 89 / 88 | 5 / 5 | 6707 / 6595 | 1.1 / 3.6 | 32.9 / 11.3 | 85 / 61 | better |
| Spase AA-A a108-0 -> Pheia Auscs AA-A d0 | 13439 | 36 / 36 | 1 / 1 | 2695 / 2694 | 0.0 / 0.1 | 0.3 / 0.6 | 38 / 2 | better |
| Spase AA-A a108-0 -> Wongi | 38200 | 102 / 102 | 5 / 5 | 7790 / 7790 | 0.0 / 0.1 | 0.4 / 0.6 | 96 / 4 | same |
| Sphiesi HX-L d7-0 -> Ood Fleau ZJ-I d9-0 | - | - / 447 | - / 19 | - / 30445 | 0.0 / 19.6 | 100.1 / 100.6 | 591 / 61 | better |
| Wongi -> Beagle Point | 65182 | 191 / 191 | 10 / 10 | 14275 / 14274 | 0.1 / 0.2 | 57.5 / 41.8 | 213 / 61 | better |
| Wongi -> Blaa Hypai AA-A a96-19 | 12712 | 38 / 38 | 1 / 1 | 2792 / 2792 | 0.1 / 0.5 | 61.6 / 1.6 | 52 / 61 | better |
| Wongi -> Colonia | 21939 | 59 / 58 | 2 / 2 | 4432 / 4351 | 0.2 / 0.1 | 1.0 / 1.7 | 59 / 61 | better |
| Wongi -> Plielou RN-R d5-27 | 12117 | 37 / 37 | 2 / 2 | 2751 / 2749 | 0.0 / 0.1 | 0.5 / 0.9 | 36 / 47 | better |
| Wongi -> Spase AA-A a108-0 | 38200 | 101 / 101 | 6 / 6 | 7726 / 7726 | 0.0 / 0.1 | 0.6 / 0.9 | 104 / 61 | same |

better 15 · same 8 · WORSE 5 of 28
