# Schema Reference

The `spacecraft_telemetry_events` table has 34 columns. The table definition
below is the canonical SQL schema as created by `images/lab-postgres-15/init.sql`.

## Table definition

```sql
CREATE TABLE spacecraft_telemetry_events (
    -- Identity
    id               VARCHAR(36)      NOT NULL PRIMARY KEY,
    spacecraft_id    VARCHAR(50)      NOT NULL,
    mission_id       VARCHAR(50)      NOT NULL,
    event_sequence   BIGINT           NOT NULL,

    -- Sensor identity
    sensor_name      VARCHAR(100)     NOT NULL,
    sensor_type      VARCHAR(50)      NOT NULL,

    -- Sensor readings (nullable — ~30% null)
    reading_int      INTEGER,
    reading_bigint   BIGINT,
    reading_decimal  DECIMAL(18, 6),
    reading_double   DOUBLE PRECISION,
    reading_bool     BOOLEAN,
    reading_text     TEXT,

    -- Event classification
    status_code      SMALLINT         NOT NULL,
    severity         VARCHAR(20)      NOT NULL,
    is_anomaly       BOOLEAN          NOT NULL,

    -- Temporal
    event_date       DATE             NOT NULL,
    event_time       TIME,                           -- ~15% null
    event_timestamp  TIMESTAMP        NOT NULL,
    received_at      TIMESTAMP,                      -- ~10% null

    -- Position and motion (nullable — ~30% null)
    latitude         DECIMAL(9, 6),
    longitude        DECIMAL(9, 6),
    altitude_km      DECIMAL(12, 3),
    velocity_kmh     DECIMAL(12, 3),

    -- Physical telemetry (nullable — ~30% null)
    temperature_c    DECIMAL(8, 3),
    radiation_level  DECIMAL(10, 5),
    battery_percent  DECIMAL(5, 2),
    payload_mass_kg  DECIMAL(10, 3),

    -- Payload and notes
    operator_notes   TEXT,                           -- ~30% null
    raw_payload_json TEXT,                           -- ~40% null
    tags             TEXT,                           -- comma-separated; ~30% null
    checksum         CHAR(64),                       -- SHA-256 hex; ~5% null

    -- Audit timestamps
    created_at       TIMESTAMP        NOT NULL,
    updated_at       TIMESTAMP        NOT NULL,
    deleted_at       TIMESTAMP                       -- ~92% null
);
```

## Indexes

```sql
CREATE INDEX idx_spacecraft  ON spacecraft_telemetry_events (spacecraft_id);
CREATE INDEX idx_mission     ON spacecraft_telemetry_events (mission_id);
CREATE INDEX idx_sensor_type ON spacecraft_telemetry_events (sensor_type);
CREATE INDEX idx_timestamp   ON spacecraft_telemetry_events (event_timestamp);
```

## Column reference

| # | Column | Postgres type | Arrow type | Nullable | Description |
|---|---|---|---|---|---|
| 1 | `id` | `VARCHAR(36)` | `Utf8` | No | UUID v4 string |
| 2 | `spacecraft_id` | `VARCHAR(50)` | `Utf8` | No | SC-001 … SC-020 |
| 3 | `mission_id` | `VARCHAR(50)` | `Utf8` | No | MISSION-ALPHA … MISSION-EPSILON |
| 4 | `event_sequence` | `BIGINT` | `Int64` | No | 1–100,000, unique per row |
| 5 | `sensor_name` | `VARCHAR(100)` | `Utf8` | No | Type-specific sensor identifier |
| 6 | `sensor_type` | `VARCHAR(50)` | `Utf8` | No | THERMAL, RADIATION, POWER, NAVIGATION, COMMS |
| 7 | `reading_int` | `INTEGER` | `Int32` | Yes (~30%) | Nullable integer reading |
| 8 | `reading_bigint` | `BIGINT` | `Int64` | Yes (~30%) | Nullable bigint reading |
| 9 | `reading_decimal` | `DECIMAL(18,6)` | `Utf8` | Yes (~30%) | Stringified decimal |
| 10 | `reading_double` | `DOUBLE PRECISION` | `Float64` | Yes (~30%) | Nullable float |
| 11 | `reading_bool` | `BOOLEAN` | `Boolean` | Yes (~30%) | Nullable boolean |
| 12 | `reading_text` | `TEXT` | `Utf8` | Yes (~30%) | Free-form string |
| 13 | `status_code` | `SMALLINT` | `Int32` | No | 0, 1, 2, or 3 |
| 14 | `severity` | `VARCHAR(20)` | `Utf8` | No | LOW, MEDIUM, HIGH, CRITICAL |
| 15 | `is_anomaly` | `BOOLEAN` | `Boolean` | No | True ~5% of rows |
| 16 | `event_date` | `DATE` | `Date32` | No | Random date in 2024 |
| 17 | `event_time` | `TIME` | `Utf8` | Yes (~15%) | Stringified time (HH:MM:SS) |
| 18 | `event_timestamp` | `TIMESTAMP` | `Timestamp(ms)` | No | Random timestamp in 2024 |
| 19 | `received_at` | `TIMESTAMP` | `Timestamp(ms)` | Yes (~10%) | ~event_timestamp + 0–60s |
| 20 | `latitude` | `DECIMAL(9,6)` | `Utf8` | Yes (~30%) | -90.0 to 90.0 |
| 21 | `longitude` | `DECIMAL(9,6)` | `Utf8` | Yes (~30%) | -180.0 to 180.0 |
| 22 | `altitude_km` | `DECIMAL(12,3)` | `Utf8` | Yes (~30%) | 200–42,000 km |
| 23 | `velocity_kmh` | `DECIMAL(12,3)` | `Utf8` | Yes (~30%) | 7,000–28,000 km/h |
| 24 | `temperature_c` | `DECIMAL(8,3)` | `Utf8` | Yes (~30%) | -270 to 1,500 °C |
| 25 | `radiation_level` | `DECIMAL(10,5)` | `Utf8` | Yes (~30%) | 0.0–100.0 |
| 26 | `battery_percent` | `DECIMAL(5,2)` | `Utf8` | Yes (~30%) | 0.0–100.0 |
| 27 | `payload_mass_kg` | `DECIMAL(10,3)` | `Utf8` | Yes (~30%) | 100–5,000 kg |
| 28 | `operator_notes` | `TEXT` | `Utf8` | Yes (~30%) | Free-form Faker sentence |
| 29 | `raw_payload_json` | `TEXT` | `Utf8` | Yes (~40%) | Simulated JSON payload string |
| 30 | `tags` | `TEXT` | `Utf8` | Yes (~30%) | Comma-separated from pool of 8 |
| 31 | `checksum` | `CHAR(64)` | `Utf8` | Yes (~5%) | SHA-256 hex of `id + event_sequence` |
| 32 | `created_at` | `TIMESTAMP` | `Timestamp(ms)` | No | Same as `event_timestamp` |
| 33 | `updated_at` | `TIMESTAMP` | `Timestamp(ms)` | No | `created_at` + 0–3,600s |
| 34 | `deleted_at` | `TIMESTAMP` | `Timestamp(ms)` | Yes (~92%) | `created_at` + 1–365 days when set |

## Data pools

**Spacecrafts (20):** SC-001 through SC-020

**Missions (5):** MISSION-ALPHA, MISSION-BETA, MISSION-GAMMA, MISSION-DELTA, MISSION-EPSILON

**Sensor types and their sensor names:**

| Type | Sensor names |
|---|---|
| THERMAL | core_temp, hull_temp, engine_temp, solar_panel_temp |
| RADIATION | gamma_detector, neutron_flux, cosmic_ray_counter, alpha_particle_sensor |
| POWER | main_bus_voltage, battery_cell_1, solar_array_output, power_regulator |
| NAVIGATION | gyroscope_x, gyroscope_y, star_tracker, inertial_nav_unit |
| COMMS | uplink_signal, downlink_signal, antenna_temperature, transponder_power |

**Severities (weighted):** LOW (60%), MEDIUM (25%), HIGH (10%), CRITICAL (5%)

**Tags pool:** `nominal`, `anomaly`, `warning`, `science`, `maintenance`, `downlink`, `uplink`, `attitude`
Each row gets 0–3 randomly chosen tags, stored as a comma-separated string.

## Why DECIMAL columns are Utf8 in Arrow

The Postgres source plugin uses `simple_query` (the Postgres text wire protocol)
instead of the binary protocol. In the text protocol, every value is sent as its
string representation — `DECIMAL(18,6)` arrives as `"1234567.890000"`.

Arrow does not have a native decimal type that covers arbitrary precision without
loss (Decimal128 has constraints that don't map cleanly to Postgres NUMERIC).
Rather than risk precision loss on high-precision financial or scientific values,
the Postgres source preserves the exact text representation and maps all
NUMERIC, DECIMAL, and TIME columns to Arrow `Utf8`.

This is a conscious design decision documented in the loadsmith source plugin.
A destination that needs DECIMAL as a native type should parse the string
representation itself.
