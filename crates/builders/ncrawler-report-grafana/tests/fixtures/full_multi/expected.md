# Grafana Report — rd-esr.nube-iiot.com
generated 2026-05-29 · mode: full · data: off · redacted
scope: --all  (2 on disk of 3 in instance inventory)

## 1. Overview
- URL:          https://rd-esr.nube-iiot.com
- Version:      7.5.17  (OSS)
- Datasources:  PostgreSQL (postgres, default) · Rubix OS (grafana-rubix-os-data-source)
- Dashboards:   3 inventory / 2 scraped       Folders: 2
- Renderer plugin: not installed  (visual/screenshot mode unavailable; chrome fallback only)

## 2. Structure / Navigation

### Folder tree
- (no folder): 1
- Sites: 1

### 0. Navigation (0. Navigation)
- All dashboards (dashlist)
- Energy portal
- Welcome (text)

### Tag index
- prod: Site Summary — Warehouse A (site-a)
- site: Site Summary — Warehouse A (site-a)

### Panel-type distribution
- dashlist: 1
- text: 2
- timeseries: 2

### Datasource usage
- PostgreSQL: 4 panel(s) across 2 dashboard(s)
- Rubix OS: 1 panel(s) across 1 dashboard(s)

## 3. Pages

### 0. Navigation — uid nav · folder: (no folder)
- 2 panels · 0 variables · datasource: PostgreSQL
- Variables: (none)
- Panels (sorted by panel id):

  | panel | id | type | datasource | value |
  |-------|----|------|-----------|-------|
  | All dashboards | 1 | dashlist | PostgreSQL | — |
  | Welcome | 2 | text | PostgreSQL | — |

### Site Summary — Warehouse A — uid site-a · folder: Sites
- 3 panels · 2 variables · datasource: PostgreSQL
- Variables: buildingRef=WH-A, hostUUID=***REDACTED***
- Panels (sorted by panel id):

  | panel | id | type | datasource | value |
  |-------|----|------|-----------|-------|
  | Electrical - MTD Usage | 2 | timeseries | PostgreSQL | — |
  | Water - MTD Usage | 5 | timeseries | Rubix OS | — |
  | Notes | 7 | text | PostgreSQL | — |

#### Electrical - MTD Usage (panel 2)

template SQL:
```sql
SELECT sum(kwh) FROM usage WHERE building='$buildingRef'
```

#### Water - MTD Usage (panel 5)

template SQL:
```sql
SELECT sum(litres) FROM water WHERE host='$hostUUID'
```
