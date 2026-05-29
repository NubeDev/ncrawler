# Grafana Report — rd-esr.nube-iiot.com
generated 2026-05-29 · mode: full · data: on (now-30d..now) · redacted
scope: --all  (2 on disk of 2 in instance inventory)

## 1. Overview
- URL:          https://rd-esr.nube-iiot.com
- Version:      7.5.17  (OSS)
- Datasources:  PostgreSQL (postgres, default) · Rubix OS (grafana-rubix-os-data-source)
- Dashboards:   2 inventory / 2 scraped       Folders: 2
- Renderer plugin: not installed  (visual/screenshot mode unavailable; chrome fallback only)

## 2. Structure / Navigation

### Folder tree
- Energy: 1
- Sites: 1

### Tag index
- energy: Energy SQL (sql-dash)
- live: Rubix Live (rubix-dash)

### Panel-type distribution
- stat: 1
- table: 1

### Datasource usage
- PostgreSQL: 1 panel(s) across 1 dashboard(s)
- Rubix OS: 1 panel(s) across 1 dashboard(s)

## 3. Pages

### Rubix Live — uid rubix-dash · folder: Energy
- 1 panels · 0 variables · datasource: Rubix OS
- Variables: (none)
- Panels (sorted by panel id):

  | panel | id | type | datasource | value |
  |-------|----|------|-----------|-------|
  | Live point | 4 | stat | Rubix OS | 42 |

#### Live point (panel 4)

template SQL:
```sql
SELECT value FROM points WHERE id = 1
```

sample returned rows:
```json
[
  {
    "value": 42
  }
]
```

### Energy SQL — uid sql-dash · folder: Sites
- 1 panels · 2 variables · datasource: PostgreSQL
- Variables: site=Lot 5, token=***REDACTED***
- Panels (sorted by panel id):

  | panel | id | type | datasource | value |
  |-------|----|------|-----------|-------|
  | Meter readings | 3 | table | PostgreSQL | 30.0 |

#### Meter readings (panel 3)

template SQL:
```sql
SELECT id, kwh FROM meters WHERE key = '***REDACTED***'
```

executed SQL (where the datasource exposes it):
```sql
SELECT id, kwh FROM meters WHERE key = '***REDACTED***' LIMIT 100
```

sample returned rows:
```json
[
  {
    "id": 1,
    "kwh": 10.5
  },
  {
    "id": 2,
    "kwh": 20.1
  },
  {
    "id": 3,
    "kwh": 30.0
  }
]
```
