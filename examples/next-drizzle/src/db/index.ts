import "dotenv/config";
import { drizzle } from "drizzle-orm/libsql";
import * as schema from "./schema";

export const db = () =>
  drizzle({ connection: process.env.PREZEL_DB_URL!, schema });
