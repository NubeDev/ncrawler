# Reference thresholds

Default triage thresholds applied when a panel does not define its own.
These are conservative starting points; a panel's embedded thresholds
always win when present.

| Signal                     | Warn (degraded) | Critical            |
| -------------------------- | --------------- | ------------------- |
| CPU utilisation            | ≥ 80%           | ≥ 95%               |
| Memory utilisation         | ≥ 85%           | ≥ 95%               |
| Disk usage                 | ≥ 80%           | ≥ 90%               |
| Request error rate (5xx)   | ≥ 1%            | ≥ 5%                |
| p95 request latency        | ≥ 500 ms        | ≥ 2000 ms           |
| Saturation / queue depth   | rising trend    | sustained at limit  |
| Availability (uptime)      | < 99.9%         | < 99%               |

Notes:

- "Near-breach" means within 10% of the warn threshold and trending
  toward it.
- A flatline at zero on a normally non-zero series is treated as a
  missing-data anomaly, not a healthy reading.
