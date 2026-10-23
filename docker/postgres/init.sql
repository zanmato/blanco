-- PostGIS and a few tables to try geometry values on. The data directory is
-- a tmpfs, so this runs every time the container starts.
CREATE EXTENSION IF NOT EXISTS postgis;

CREATE SCHEMA gis;

-- geography points, which always carry SRID 4326.
CREATE TABLE gis.cities (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL,
    country text NOT NULL,
    location geography(Point, 4326) NOT NULL
);
CREATE INDEX cities_location_idx ON gis.cities USING gist (location);

INSERT INTO gis.cities (name, country, location) VALUES
    ('Stockholm', 'Sweden', 'SRID=4326;POINT(18.0686 59.3293)'),
    ('Gothenburg', 'Sweden', 'SRID=4326;POINT(11.9746 57.7089)'),
    ('Malmö', 'Sweden', 'SRID=4326;POINT(13.0038 55.605)'),
    ('Oslo', 'Norway', 'SRID=4326;POINT(10.7522 59.9139)'),
    ('Copenhagen', 'Denmark', 'SRID=4326;POINT(12.5683 55.6761)'),
    ('Helsinki', 'Finland', 'SRID=4326;POINT(24.9384 60.1699)'),
    ('New York', 'United States', 'SRID=4326;POINT(-73.9857 40.7484)'),
    ('Tokyo', 'Japan', 'SRID=4326;POINT(139.6917 35.6895)');

-- An unconstrained geometry column with every simple feature type, mixed
-- SRIDs, Z and M values, an empty geometry, a curve that Blanco does not
-- decode and so shows as hex, and a NULL.
CREATE TABLE gis.shapes (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    description text NOT NULL,
    shape geometry
);

INSERT INTO gis.shapes (description, shape) VALUES
    ('Point with an SRID', 'SRID=4326;POINT(18.0686 59.3293)'),
    ('Point without an SRID', 'POINT(1 2)'),
    ('Point with Z', 'SRID=4326;POINT Z (18.0686 59.3293 28)'),
    ('Point with M', 'POINT M (1 2 3)'),
    ('Point with Z and M', 'POINT ZM (1 2 3 4)'),
    ('Empty point', 'POINT EMPTY'),
    ('Line string', 'SRID=4326;LINESTRING(18.0686 59.3293,11.9746 57.7089,13.0038 55.605)'),
    ('Line string with Z', 'LINESTRING Z (0 0 1,1 1 2)'),
    ('Polygon with a hole, SWEREF 99 TM', 'SRID=3006;POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,2 1,2 2,1 1))'),
    ('Multi point', 'MULTIPOINT((0 0),(1 1),(2 0))'),
    ('Multi line string', 'MULTILINESTRING((0 0,1 1),(2 2,3 3))'),
    ('Multi polygon', 'MULTIPOLYGON(((0 0,1 0,1 1,0 0)),((2 2,3 2,3 3,2 2)))'),
    ('Geometry collection', 'GEOMETRYCOLLECTION(POINT(1 2),LINESTRING(0 0,1 1))'),
    ('Circular string, shown as hex', 'CIRCULARSTRING(0 0,1 1,2 0)'),
    ('No value', NULL);

-- Polygons in a column whose type fixes the shape and the SRID.
CREATE TABLE gis.regions (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL,
    area geometry(Polygon, 4326) NOT NULL
);

INSERT INTO gis.regions (name, area) VALUES
    ('Stockholm inner city', 'SRID=4326;POLYGON((18.03 59.31,18.11 59.31,18.11 59.35,18.03 59.35,18.03 59.31))'),
    ('Gothenburg centre', 'SRID=4326;POLYGON((11.93 57.69,12.01 57.69,12.01 57.72,11.93 57.72,11.93 57.69))');
