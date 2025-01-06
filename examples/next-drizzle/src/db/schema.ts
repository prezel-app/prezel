import { int, sqliteTable } from "drizzle-orm/sqlite-core";

export const claps = sqliteTable("claps", {
  id: int().primaryKey(),
  count: int().notNull().default(0),
});
