// The docs site is static. Robodev needs a database with at least one table,
// so this declares one small placeholder that nothing reads or writes.
import { defineDatabase, pgTable, text } from "@robodev-ai/sdk";

export const siteMeta = pgTable("site_meta", {
  key: text("key").primaryKey(),
  value: text("value"),
});

export default defineDatabase({
  name: "lightningpanda_docs",
  tables: { siteMeta },
});
