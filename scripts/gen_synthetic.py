#!/usr/bin/env python3
"""生成合成 LEO 物体的 OMM JSON,用于前端/服务端的规模测试。

用法:python3 scripts/gen_synthetic.py 20000 > /tmp/synth.json
历元为 2024-01-01,配合 `orbitscope serve --sim-time 2024-01-01T12:30:00Z` 使用。
"""
import json
import sys

n = int(sys.argv[1]) if len(sys.argv) > 1 else 20000
types = ["PAYLOAD", "ROCKET BODY", "DEBRIS", "DEBRIS", "DEBRIS", "UNKNOWN"]
objs = []
for i in range(n):
    f = i / n
    objs.append({
        "OBJECT_NAME": f"SYN {i}", "OBJECT_ID": f"2020-{i % 999 + 1:03d}A",
        "EPOCH": f"2024-01-01T{i % 12:02d}:00:00",
        "MEAN_MOTION": 12.6 + 3.4 * ((i * 31 % 1000) / 1000),
        "ECCENTRICITY": 0.0005 + 0.01 * ((i * 13 % 97) / 97),
        "INCLINATION": 20 + 80 * f, "RA_OF_ASC_NODE": (i * 7.3) % 360,
        "ARG_OF_PERICENTER": (i * 3.1) % 360, "MEAN_ANOMALY": (i * 11.7) % 360,
        "NORAD_CAT_ID": 100000 + i, "BSTAR": 0.0001, "MEAN_MOTION_DOT": 0, "MEAN_MOTION_DDOT": 0,
        "OBJECT_TYPE": types[i % 6], "RCS_SIZE": ["SMALL", "MEDIUM", "LARGE"][i % 3],
    })
json.dump(objs, sys.stdout)
