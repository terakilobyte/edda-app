# Planner matrix, before and after (2026-10-19, final, seconds gate corrected): the shipped planner and the R2 branch head vs branch planner/r3 (the feasibility witness, its segments and the descent seed, LazySP repair, the bound-gap stop, the quality floor, the heavy slice, the witness in the plot's judging currency, the leg precheck), 28 routes, warm, 100 s, seconds judge

Rows from docs/benches/2026-10-17-planner-matrix-{before,before3,thin6,thin6-3}.jsonl and 2026-10-18-planner-matrix-{w21,w21-3}.jsonl via knobs/planner_before_after.py --judge seconds. Columns: route, straight ly, jumps b/a, stops b/a, pilot seconds b/a, first route s b/a, wall s b/a, the external reference's jumps / seconds (for the eye), verdict. All 28 routes have a route on both thread counts; the two Byoi -> Jongou reverse crossings are 444 / 542-543 jumps (the night of the 17th: 910 / 1,054; the reference column 477 / 516).

## Three threads (the box's long lane): the shipped planner (before3) -> w21-3

| route | straight ly | jumps b/a | stops b/a | pilot s b/a | first s b/a | wall s b/a | ref jumps / s | verdict |
|---|---|---|---|---|---|---|---|---|
| Beagle Point -> Sol | 65279 | 197 / 197 | 9 / 9 | 14597 / 14592 | 0.1 / 0.1 | 1.7 / 1.1 | 198 / 29 | better |
| Beagle Point -> Wongi | 65182 | 196 / 196 | 10 / 10 | 14555 / 14518 | 0.1 / 0.1 | 1.3 / 1.1 | 198 / 28 | better |
| Blaa Hypai AA-A a96-19 -> Wongi | 12712 | 36 / 36 | 1 / 1 | 2674 / 2674 | 0.0 / 0.1 | 0.2 / 0.5 | 37 / 10 | same |
| Byoi Fraae AT-U d2-1 -> Jongou AA-A d0 | - | - / 444 | - / 18 | - / 30340 | 0.0 / 0.3 | 100.1 / 102.5 | 477 / 61 | better |
| Byoi Fraae CQ-G d10-0 -> Jongou XM-W d1-0 | - | - / 542 | - / 23 | - / 35832 | 0.0 / 0.5 | 100.2 / 102.9 | 516 / 61 | better |
| Byoomao AA-A c1 -> Sol | 24621 | 62 / 62 | 3 / 3 | 4708 / 4694 | 0.0 / 0.1 | 0.6 / 1.2 | 61 / 2 | better |
| Colonia -> Sol | 22000 | 55 / 55 | 2 / 2 | 4161 / 4161 | 0.1 / 0.1 | 1.0 / 2.3 | 54 / 15 | WORSE |
| Colonia -> Spase AA-A a108-0 | 31440 | 91 / 88 | 4 / 4 | 6746 / 6579 | 0.8 / 0.4 | 37.0 / 2.7 | 90 / 61 | better |
| Colonia -> Wongi | 21939 | 54 / 54 | 2 / 2 | 4096 / 4096 | 0.1 / 0.1 | 1.1 / 2.4 | 54 / 16 | WORSE |
| Jongou AA-A d0 -> Byoi Fraae AT-U d2-1 | 85526 | 396 / 396 | 17 / 17 | 27520 / 27517 | 1.0 / 1.2 | 22.8 / 53.0 | 492 / 62 | better |
| Jongou XM-W d1-0 -> Byoi Fraae CQ-G d10-0 | - | - / 401 | - / 17 | - / 27974 | 0.0 / 4.1 | 100.1 / 69.2 | 530 / 61 | better |
| Ood Fleau ZJ-I d9-0 -> Sphiesi HX-L d7-0 | - | - / 410 | - / 18 | - / 28432 | 0.0 / 2.3 | 100.1 / 100.8 | 451 / 61 | better |
| Pheia Auscs AA-A d0 -> Spase AA-A a108-0 | 13439 | 37 / 37 | 1 / 1 | 2760 / 2760 | 0.0 / 0.1 | 0.4 / 0.9 | 38 / 1 | same |
| Plielou RN-R d5-27 -> Wongi | 12117 | 36 / 37 | 1 / 1 | 2648 / 2643 | 0.0 / 0.1 | 0.3 / 0.6 | 36 / 3 | better |
| Sagittarius A* -> Sol | 25900 | 72 / 73 | 4 / 3 | 5414 / 5427 | 0.9 / 1.0 | 5.3 / 7.0 | 70 / 38 | WORSE |
| Sol -> Beagle Point | 65279 | 191 / 191 | 10 / 10 | 14273 / 14273 | 0.1 / 0.7 | 50.3 / 38.6 | 205 / 61 | better |
| Sol -> Byoomao AA-A c1 | 24621 | 64 / 64 | 3 / 3 | 4827 / 4871 | 0.0 / 0.1 | 0.6 / 1.2 | 74 / 61 | WORSE |
| Sol -> Colonia | 22000 | 58 / 57 | 3 / 3 | 4360 / 4320 | 0.2 / 0.1 | 1.1 / 5.6 | 60 / 38 | better |
| Sol -> Sagittarius A* | 25900 | 68 / 68 | 4 / 4 | 5140 / 5145 | 0.1 / 0.2 | 1.3 / 3.2 | 78 / 60 | WORSE |
| Spase AA-A a108-0 -> Colonia | 31440 | 89 / 91 | 5 / 5 | 6707 / 6830 | 1.1 / 1.0 | 32.9 / 12.3 | 85 / 61 | WORSE |
| Spase AA-A a108-0 -> Pheia Auscs AA-A d0 | 13439 | 36 / 36 | 1 / 1 | 2695 / 2694 | 0.0 / 0.1 | 0.3 / 0.8 | 38 / 2 | better |
| Spase AA-A a108-0 -> Wongi | 38200 | 102 / 102 | 5 / 5 | 7790 / 7790 | 0.0 / 0.1 | 0.4 / 0.7 | 96 / 4 | same |
| Sphiesi HX-L d7-0 -> Ood Fleau ZJ-I d9-0 | - | - / 462 | - / 20 | - / 32436 | 0.0 / 0.5 | 100.1 / 36.6 | 591 / 61 | better |
| Wongi -> Beagle Point | 65182 | 191 / 191 | 10 / 10 | 14275 / 14274 | 0.1 / 0.7 | 57.5 / 47.7 | 213 / 61 | better |
| Wongi -> Blaa Hypai AA-A a96-19 | 12712 | 38 / 38 | 1 / 1 | 2792 / 2792 | 0.1 / 0.3 | 61.6 / 10.6 | 52 / 61 | better |
| Wongi -> Colonia | 21939 | 59 / 58 | 2 / 2 | 4432 / 4350 | 0.2 / 0.1 | 1.0 / 5.5 | 59 / 61 | better |
| Wongi -> Plielou RN-R d5-27 | 12117 | 37 / 38 | 2 / 1 | 2751 / 2775 | 0.0 / 0.1 | 0.5 / 0.7 | 36 / 47 | WORSE |
| Wongi -> Spase AA-A a108-0 | 38200 | 101 / 101 | 6 / 6 | 7726 / 7726 | 0.0 / 0.1 | 0.6 / 1.1 | 104 / 61 | same |

better 17 · same 4 · WORSE 7 of 28

## Three threads: the R2 branch head (thin6-3) -> w21-3

| route | straight ly | jumps b/a | stops b/a | pilot s b/a | first s b/a | wall s b/a | ref jumps / s | verdict |
|---|---|---|---|---|---|---|---|---|
| Beagle Point -> Sol | 65279 | 197 / 197 | 9 / 9 | 14592 / 14592 | 0.1 / 0.1 | 0.9 / 1.1 | 198 / 29 | same |
| Beagle Point -> Wongi | 65182 | 196 / 196 | 10 / 10 | 14518 / 14518 | 0.1 / 0.1 | 1.0 / 1.1 | 198 / 28 | same |
| Blaa Hypai AA-A a96-19 -> Wongi | 12712 | 36 / 36 | 1 / 1 | 2674 / 2674 | 0.1 / 0.1 | 0.4 / 0.5 | 37 / 10 | same |
| Byoi Fraae AT-U d2-1 -> Jongou AA-A d0 | - | - / 444 | - / 18 | - / 30340 | 0.0 / 0.3 | 101.1 / 102.5 | 477 / 61 | better |
| Byoi Fraae CQ-G d10-0 -> Jongou XM-W d1-0 | - | - / 542 | - / 23 | - / 35832 | 0.0 / 0.5 | 101.1 / 102.9 | 516 / 61 | better |
| Byoomao AA-A c1 -> Sol | 24621 | 62 / 62 | 3 / 3 | 4714 / 4694 | 0.1 / 0.1 | 0.7 / 1.2 | 61 / 2 | better |
| Colonia -> Sol | 22000 | 56 / 55 | 2 / 2 | 4248 / 4161 | 0.1 / 0.1 | 1.0 / 2.3 | 54 / 15 | better |
| Colonia -> Spase AA-A a108-0 | 31440 | 88 / 88 | 4 / 4 | 6582 / 6579 | 6.2 / 0.4 | 14.1 / 2.7 | 90 / 61 | better |
| Colonia -> Wongi | 21939 | 55 / 54 | 3 / 2 | 4201 / 4096 | 0.1 / 0.1 | 1.1 / 2.4 | 54 / 16 | better |
| Jongou AA-A d0 -> Byoi Fraae AT-U d2-1 | 85526 | 396 / 396 | 17 / 17 | 27517 / 27517 | 1.1 / 1.2 | 54.0 / 53.0 | 492 / 62 | same |
| Jongou XM-W d1-0 -> Byoi Fraae CQ-G d10-0 | 87012 | 401 / 401 | 17 / 17 | 27974 / 27974 | 14.2 / 4.1 | 100.9 / 69.2 | 530 / 61 | better |
| Ood Fleau ZJ-I d9-0 -> Sphiesi HX-L d7-0 | 84599 | 412 / 410 | 17 / 18 | 28490 / 28432 | 6.6 / 2.3 | 102.7 / 100.8 | 451 / 61 | WORSE |
| Pheia Auscs AA-A d0 -> Spase AA-A a108-0 | 13439 | 37 / 37 | 1 / 1 | 2760 / 2760 | 0.1 / 0.1 | 0.7 / 0.9 | 38 / 1 | same |
| Plielou RN-R d5-27 -> Wongi | 12117 | 37 / 37 | 1 / 1 | 2643 / 2643 | 0.1 / 0.1 | 0.6 / 0.6 | 36 / 3 | same |
| Sagittarius A* -> Sol | 25900 | 73 / 73 | 3 / 3 | 5427 / 5427 | 1.9 / 1.0 | 6.2 / 7.0 | 70 / 38 | same |
| Sol -> Beagle Point | 65279 | 191 / 191 | 10 / 10 | 14273 / 14273 | 0.2 / 0.7 | 33.6 / 38.6 | 205 / 61 | WORSE |
| Sol -> Byoomao AA-A c1 | 24621 | 64 / 64 | 3 / 3 | 4871 / 4871 | 0.1 / 0.1 | 0.9 / 1.2 | 74 / 61 | same |
| Sol -> Colonia | 22000 | 58 / 57 | 3 / 3 | 4358 / 4320 | 0.1 / 0.1 | 1.4 / 5.6 | 60 / 38 | better |
| Sol -> Sagittarius A* | 25900 | 68 / 68 | 4 / 4 | 5146 / 5145 | 0.1 / 0.2 | 1.0 / 3.2 | 78 / 60 | better |
| Spase AA-A a108-0 -> Colonia | 31440 | 88 / 91 | 5 / 5 | 6595 / 6830 | 3.7 / 1.0 | 11.6 / 12.3 | 85 / 61 | WORSE |
| Spase AA-A a108-0 -> Pheia Auscs AA-A d0 | 13439 | 36 / 36 | 1 / 1 | 2694 / 2694 | 0.1 / 0.1 | 0.6 / 0.8 | 38 / 2 | same |
| Spase AA-A a108-0 -> Wongi | 38200 | 102 / 102 | 5 / 5 | 7790 / 7790 | 0.1 / 0.1 | 0.6 / 0.7 | 96 / 4 | same |
| Sphiesi HX-L d7-0 -> Ood Fleau ZJ-I d9-0 | 84599 | 447 / 462 | 19 / 20 | 30445 / 32436 | 20.3 / 0.5 | 100.6 / 36.6 | 591 / 61 | WORSE |
| Wongi -> Beagle Point | 65182 | 191 / 191 | 10 / 10 | 14274 / 14274 | 0.2 / 0.7 | 44.2 / 47.7 | 213 / 61 | same |
| Wongi -> Blaa Hypai AA-A a96-19 | 12712 | 38 / 38 | 1 / 1 | 2792 / 2792 | 0.5 / 0.3 | 1.6 / 10.6 | 52 / 61 | WORSE |
| Wongi -> Colonia | 21939 | 58 / 58 | 2 / 2 | 4351 / 4350 | 0.1 / 0.1 | 1.7 / 5.5 | 59 / 61 | better |
| Wongi -> Plielou RN-R d5-27 | 12117 | 37 / 38 | 2 / 1 | 2749 / 2775 | 0.1 / 0.1 | 0.9 / 0.7 | 36 / 47 | WORSE |
| Wongi -> Spase AA-A a108-0 | 38200 | 101 / 101 | 6 / 6 | 7726 / 7726 | 0.1 / 0.1 | 1.0 / 1.1 | 104 / 61 | same |

better 10 · same 12 · WORSE 6 of 28

## PC, 24 threads: the shipped planner (before) -> w21

| route | straight ly | jumps b/a | stops b/a | pilot s b/a | first s b/a | wall s b/a | ref jumps / s | verdict |
|---|---|---|---|---|---|---|---|---|
| Beagle Point -> Sol | 65279 | 197 / 197 | 9 / 9 | 14597 / 14592 | 0.1 / 0.2 | 1.5 / 1.0 | 198 / 29 | better |
| Beagle Point -> Wongi | 65182 | 196 / 196 | 10 / 10 | 14555 / 14518 | 0.3 / 0.2 | 1.6 / 1.0 | 198 / 28 | better |
| Blaa Hypai AA-A a96-19 -> Wongi | 12712 | 36 / 36 | 1 / 1 | 2674 / 2674 | 0.1 / 0.1 | 0.2 / 0.4 | 37 / 10 | same |
| Byoi Fraae AT-U d2-1 -> Jongou AA-A d0 | - | - / 444 | - / 18 | - / 30340 | 0.0 / 0.4 | 100.2 / 78.5 | 477 / 61 | better |
| Byoi Fraae CQ-G d10-0 -> Jongou XM-W d1-0 | - | - / 543 | - / 23 | - / 35805 | 0.0 / 0.9 | 100.2 / 71.8 | 516 / 61 | better |
| Byoomao AA-A c1 -> Sol | 24621 | 62 / 62 | 3 / 3 | 4708 / 4694 | 0.1 / 0.1 | 0.6 / 1.0 | 61 / 2 | better |
| Colonia -> Sol | 22000 | 55 / 55 | 2 / 2 | 4170 / 4161 | 0.2 / 0.1 | 0.9 / 1.5 | 54 / 15 | better |
| Colonia -> Spase AA-A a108-0 | 31440 | 91 / 88 | 4 / 4 | 6746 / 6579 | 1.4 / 0.5 | 52.4 / 2.2 (same-day) | 90 / 61 | better |
| Colonia -> Wongi | 21939 | 54 / 54 | 2 / 2 | 4096 / 4096 | 0.4 / 0.2 | 1.2 / 1.5 | 54 / 16 | same |
| Jongou AA-A d0 -> Byoi Fraae AT-U d2-1 | 85526 | 396 / 396 | 17 / 17 | 27520 / 27517 | 1.3 / 1.9 | 100.2 / 100.3 | 492 / 62 | better |
| Jongou XM-W d1-0 -> Byoi Fraae CQ-G d10-0 | 87012 | 402 / 401 | 17 / 17 | 28028 / 27974 | 100.0 / 14.8 | 108.9 / 22.9 | 530 / 61 | better |
| Ood Fleau ZJ-I d9-0 -> Sphiesi HX-L d7-0 | - | - / 410 | - / 18 | - / 28432 | 0.0 / 2.2 | 100.3 / 100.9 | 451 / 61 | better |
| Pheia Auscs AA-A d0 -> Spase AA-A a108-0 | 13439 | 37 / 37 | 1 / 1 | 2760 / 2760 | 0.1 / 0.1 | 0.4 / 0.8 | 38 / 1 | same |
| Plielou RN-R d5-27 -> Wongi | 12117 | 36 / 37 | 1 / 1 | 2648 / 2643 | 0.1 / 0.1 | 0.2 / 0.4 | 36 / 3 | better |
| Sagittarius A* -> Sol | 25900 | 73 / 73 | 3 / 3 | 5428 / 5427 | 2.0 / 1.0 | 6.3 / 6.0 | 70 / 38 | better |
| Sol -> Beagle Point | 65279 | 191 / 191 | 10 / 10 | 14273 / 14273 | 0.3 / 0.2 | 77.4 / 15.6 (same-day) | 205 / 61 | better |
| Sol -> Byoomao AA-A c1 | 24621 | 64 / 64 | 3 / 3 | 4827 / 4871 | 0.2 / 0.1 | 0.6 / 1.1 | 74 / 61 | WORSE |
| Sol -> Colonia | 22000 | 58 / 57 | 3 / 3 | 4360 / 4320 | 0.1 / 0.2 | 1.0 / 3.4 | 60 / 38 | better |
| Sol -> Sagittarius A* | 25900 | 70 / 68 | 3 / 4 | 5283 / 5145 | 0.1 / 0.2 | 0.8 / 2.0 | 78 / 60 | WORSE |
| Spase AA-A a108-0 -> Colonia | 31440 | 87 / 91 | 5 / 5 | 6516 / 6830 | 2.0 / 0.8 | 7.2 / 4.9 | 85 / 61 | WORSE |
| Spase AA-A a108-0 -> Pheia Auscs AA-A d0 | 13439 | 36 / 36 | 1 / 1 | 2695 / 2694 | 0.0 / 0.1 | 0.3 / 0.7 | 38 / 2 | better |
| Spase AA-A a108-0 -> Wongi | 38200 | 102 / 102 | 5 / 5 | 7790 / 7790 | 0.1 / 0.1 | 0.4 / 0.5 | 96 / 4 | same |
| Sphiesi HX-L d7-0 -> Ood Fleau ZJ-I d9-0 | - | - / 447 | - / 19 | - / 30445 | 0.0 / 1.6 | 100.2 / 42.7 | 591 / 61 | better |
| Wongi -> Beagle Point | 65182 | 191 / 191 | 10 / 10 | 14275 / 14274 | 0.3 / 0.2 | 80.3 / 82.1 (same-day) | 213 / 61 | better |
| Wongi -> Blaa Hypai AA-A a96-19 | 12712 | 38 / 38 | 1 / 1 | 2792 / 2792 | 0.2 / 0.2 | 1.0 / 7.0 | 52 / 61 | WORSE |
| Wongi -> Colonia | 21939 | 58 / 58 | 2 / 2 | 4351 / 4350 | 0.1 / 0.2 | 1.4 / 3.3 | 59 / 61 | better |
| Wongi -> Plielou RN-R d5-27 | 12117 | 37 / 38 | 2 / 1 | 2751 / 2775 | 0.1 / 0.1 | 0.6 / 0.6 | 36 / 47 | WORSE |
| Wongi -> Spase AA-A a108-0 | 38200 | 101 / 101 | 6 / 6 | 7726 / 7726 | 0.1 / 0.1 | 0.6 / 1.0 | 104 / 61 | same |

better 18 · same 5 · WORSE 5 of 28

## PC, 24 threads: the R2 branch head (thin6) -> w21

| route | straight ly | jumps b/a | stops b/a | pilot s b/a | first s b/a | wall s b/a | ref jumps / s | verdict |
|---|---|---|---|---|---|---|---|---|
| Beagle Point -> Sol | 65279 | 197 / 197 | 9 / 9 | 14592 / 14592 | 0.2 / 0.2 | 0.6 / 1.0 | 198 / 29 | same |
| Beagle Point -> Wongi | 65182 | 196 / 196 | 10 / 10 | 14518 / 14518 | 0.2 / 0.2 | 0.6 / 1.0 | 198 / 28 | same |
| Blaa Hypai AA-A a96-19 -> Wongi | 12712 | 36 / 36 | 1 / 1 | 2674 / 2674 | 0.1 / 0.1 | 0.3 / 0.4 | 37 / 10 | same |
| Byoi Fraae AT-U d2-1 -> Jongou AA-A d0 | - | - / 444 | - / 18 | - / 30340 | 0.0 / 0.4 | 81.4 / 78.5 | 477 / 61 | better |
| Byoi Fraae CQ-G d10-0 -> Jongou XM-W d1-0 | - | - / 543 | - / 23 | - / 35805 | 0.0 / 0.9 | 85.5 / 71.8 | 516 / 61 | better |
| Byoomao AA-A c1 -> Sol | 24621 | 62 / 62 | 3 / 3 | 4694 / 4694 | 0.1 / 0.1 | 0.8 / 1.0 | 61 / 2 | same |
| Colonia -> Sol | 22000 | 55 / 55 | 2 / 2 | 4161 / 4161 | 0.1 / 0.1 | 1.0 / 1.5 | 54 / 15 | same |
| Colonia -> Spase AA-A a108-0 | 31440 | 88 / 88 | 4 / 4 | 6582 / 6579 | 1.0 / 0.5 | 5.1 / 2.2 | 90 / 61 | better |
| Colonia -> Wongi | 21939 | 54 / 54 | 2 / 2 | 4096 / 4096 | 0.1 / 0.2 | 0.9 / 1.5 | 54 / 16 | same |
| Jongou AA-A d0 -> Byoi Fraae AT-U d2-1 | 85526 | 396 / 396 | 17 / 17 | 27517 / 27517 | 1.7 / 1.9 | 100.3 / 100.3 | 492 / 62 | same |
| Jongou XM-W d1-0 -> Byoi Fraae CQ-G d10-0 | 87012 | 401 / 401 | 17 / 17 | 27974 / 27974 | 14.4 / 14.8 | 58.0 / 22.9 | 530 / 61 | better |
| Ood Fleau ZJ-I d9-0 -> Sphiesi HX-L d7-0 | 84599 | 412 / 410 | 17 / 18 | 28490 / 28432 | 29.0 / 2.2 | 102.6 / 100.9 | 451 / 61 | WORSE |
| Pheia Auscs AA-A d0 -> Spase AA-A a108-0 | 13439 | 37 / 37 | 1 / 1 | 2760 / 2760 | 0.1 / 0.1 | 0.7 / 0.8 | 38 / 1 | same |
| Plielou RN-R d5-27 -> Wongi | 12117 | 37 / 37 | 1 / 1 | 2643 / 2643 | 0.1 / 0.1 | 0.4 / 0.4 | 36 / 3 | same |
| Sagittarius A* -> Sol | 25900 | 73 / 73 | 3 / 3 | 5434 / 5427 | 1.0 / 1.0 | 2.9 / 6.0 | 70 / 38 | better |
| Sol -> Beagle Point | 65279 | 191 / 191 | 10 / 10 | 14273 / 14273 | 0.2 / 0.2 | 77.1 / 15.6 | 205 / 61 | better |
| Sol -> Byoomao AA-A c1 | 24621 | 64 / 64 | 3 / 3 | 4871 / 4871 | 0.1 / 0.1 | 0.9 / 1.1 | 74 / 61 | same |
| Sol -> Colonia | 22000 | 58 / 57 | 3 / 3 | 4358 / 4320 | 0.2 / 0.2 | 1.2 / 3.4 | 60 / 38 | better |
| Sol -> Sagittarius A* | 25900 | 68 / 68 | 4 / 4 | 5146 / 5145 | 0.2 / 0.2 | 1.0 / 2.0 | 78 / 60 | better |
| Spase AA-A a108-0 -> Colonia | 31440 | 87 / 91 | 5 / 5 | 6516 / 6830 | 1.2 / 0.8 | 6.1 / 4.9 | 85 / 61 | WORSE |
| Spase AA-A a108-0 -> Pheia Auscs AA-A d0 | 13439 | 36 / 36 | 1 / 1 | 2694 / 2694 | 0.1 / 0.1 | 0.6 / 0.7 | 38 / 2 | same |
| Spase AA-A a108-0 -> Wongi | 38200 | 102 / 102 | 5 / 5 | 7790 / 7790 | 0.1 / 0.1 | 0.5 / 0.5 | 96 / 4 | same |
| Sphiesi HX-L d7-0 -> Ood Fleau ZJ-I d9-0 | 84599 | 447 / 447 | 19 / 19 | 30445 / 30445 | 33.4 / 1.6 | 101.5 / 42.7 | 591 / 61 | better |
| Wongi -> Beagle Point | 65182 | 191 / 191 | 10 / 10 | 14274 / 14274 | 0.2 / 0.2 | 76.9 / 82.1 | 213 / 61 | same |
| Wongi -> Blaa Hypai AA-A a96-19 | 12712 | 38 / 38 | 1 / 1 | 2792 / 2792 | 0.2 / 0.2 | 1.2 / 7.0 | 52 / 61 | WORSE |
| Wongi -> Colonia | 21939 | 58 / 58 | 2 / 2 | 4351 / 4350 | 0.2 / 0.2 | 1.4 / 3.3 | 59 / 61 | better |
| Wongi -> Plielou RN-R d5-27 | 12117 | 37 / 38 | 2 / 1 | 2749 / 2775 | 0.1 / 0.1 | 0.8 / 0.6 | 36 / 47 | WORSE |
| Wongi -> Spase AA-A a108-0 | 38200 | 101 / 101 | 6 / 6 | 7726 / 7726 | 0.1 / 0.1 | 0.8 / 1.0 | 104 / 61 | same |

better 10 · same 14 · WORSE 4 of 28
