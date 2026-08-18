CREATE CONSTRAINT projection_cursor_identity IF NOT EXISTS
FOR (c:ProjectionCursor) REQUIRE (c.entity_type, c.entity_key) IS UNIQUE;
