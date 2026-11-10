import { z } from "zod";

// Shapes of the data the tool publishes. Everything read from the store or
// received on /api/ingest is parsed with these, so pages can trust the types.

const num = z.number().finite();
const ms = z.number().int().nonnegative();

export const EventSchema = z.object({
  id: z.string().min(1).max(300),
  ms,
  asset: z.string().max(60),
  kind: z.string().max(20),
  title: z.string().max(500),
  text: z.string().max(8000),
  notify: z.boolean(),
  /** How much it matters, 1 to 3. */
  level: z.number().int().min(1).max(3).optional(),
  /** News only: which way it pushes prices. */
  lean: z.enum(["alta", "queda", "incerto"]).nullable().optional(),
  market: z.enum(["cripto", "acoes"]).optional(),
})
  // Events saved before these fields existed get the same defaults the tool uses.
  .transform((e) => ({
    ...e,
    level: e.level ?? (e.notify ? (e.kind === "stormer" ? 3 : 2) : 1),
    lean: e.lean ?? null,
    market: e.market ?? (["BTC", "ETH", "SOL", "mercado"].includes(e.asset) ? "cripto" : "acoes"),
  }));

const MoodItemSchema = z.object({
  title: z.string().max(300),
  link: z.string().url().max(2000).refine((u) => u.startsWith("https://") || u.startsWith("http://")),
  source: z.string().max(100),
  ms,
  kind: z.string().max(60),
  lean: z.enum(["alta", "queda", "incerto"]),
  level: z.number().int().min(1).max(3),
});

const MarketMoodSchema = z.object({
  hours: z.number().int().positive(),
  up: z.number().int().nonnegative(),
  down: z.number().int().nonnegative(),
  unclear: z.number().int().nonnegative(),
  weight: num,
  score: num,
  items: z.array(MoodItemSchema).max(30),
});

export const MoodSchema = z.object({ cripto: MarketMoodSchema, acoes: MarketMoodSchema });

const side = z.enum(["long", "short"]);

export const SetupSchema = z.object({
  key: z.string().max(200),
  side,
  status: z.enum(["armado", "aberto", "alvo", "stop", "prazo", "cancelado"]),
  candidate: z.boolean(),
  wyckoff: z.boolean(),
  climax: z.boolean(),
  entry: num,
  stop: num,
  target: num,
  rr: num,
  swept_level: num,
  sweep_extreme: num,
  choch_level: num,
  sweep_ms: ms,
  choch_ms: ms,
  valid_until_ms: ms,
  fill_ms: ms.nullable(),
  exit_ms: ms.nullable(),
  exit_price: num.nullable(),
  r: num.nullable(),
});

export const PendingSchema = z.object({
  key: z.string().max(200),
  side,
  candidate: z.boolean(),
  wyckoff: z.boolean(),
  climax: z.boolean(),
  swept_level: num,
  sweep_extreme: num,
  choch_level: num,
  sweep_ms: ms,
  deadline_ms: ms,
});

export const FrameSchema = z.object({
  tf: z.enum(["1h", "4h", "1d"]),
  label: z.string().max(40),
  liquidity: z.object({ above: num.nullable(), below: num.nullable() }),
  setups: z.array(SetupSchema).max(200),
  pending: z.array(PendingSchema).max(20),
});

export const StormerSchema = z.object({
  key: z.string().max(200),
  tf: z.enum(["1d", "1w"]),
  pattern: z.string().max(40),
  side,
  role: z.enum(["candidato", "observacao", "acompanhar"]),
  status: z.enum(["sinal", "aberto", "alvo", "stop", "prazo"]),
  trigger: num,
  stop: num,
  target: num,
  signal_ms: ms,
  deadline_ms: ms.nullable(),
  entry_ms: ms.nullable(),
  exit_ms: ms.nullable(),
  exit: num.nullable(),
  r: num.nullable(),
});

export const AssetSchema = z.object({
  symbol: z.string().regex(/^[A-Z0-9]{2,15}USDT$/),
  name: z.string().max(15),
  price: num.nullable(),
  change_24h: num.nullable(),
  frames: z.array(FrameSchema).max(5),
  stormer: z.array(StormerSchema).max(200).default([]),
});

export const BookTradeSchema = z.object({
  key: z.string().max(200),
  asset: z.string().max(20),
  side,
  fill_ms: ms,
  exit_ms: ms,
  entry: num,
  stop: num,
  exit: num,
  r: num,
  status: z.string().max(20),
  equity_after: num,
});

export const BookSchema = z.object({
  tf: z.string().max(5),
  label: z.string().max(40),
  start_ms: ms.nullable(),
  capital: num,
  equity: num,
  trades: z.array(BookTradeSchema).max(5000),
});

export const LiveSchema = z.object({
  version: z.literal(1),
  generated_ms: ms,
  assets: z.array(AssetSchema).max(10),
  books: z.array(BookSchema).max(5),
  events: z.array(EventSchema).max(1000),
  mood: MoodSchema.nullable().default(null),
});

const ZoneSchema = z.object({ low: num, high: num, usd: num }).nullable();

export const DailySchema = z.object({
  version: z.literal(1),
  generated_ms: ms,
  date: z.string().regex(/^\d{4}-\d{2}-\d{2}$/),
  sections: z.array(z.string().max(8000)).max(10),
  assets: z
    .array(
      z.object({
        name: z.string().max(15),
        close: num,
        change: num,
        trend: z.string().max(20),
        previous_trend: z.string().max(20).nullable(),
        positioning: z
          .object({
            long_share: num,
            long_share_before: num,
            top_long_share: num.nullable(),
            oi_change_24h: num.nullable(),
            longs_zone: ZoneSchema,
            shorts_zone: ZoneSchema,
            exchanges: z.array(z.string().max(20)).max(10),
          })
          .nullable(),
      }),
    )
    .max(10),
});

const CandleSchema = z.tuple([ms, num, num, num, num]);

export const MarketAssetSchema = z.object({
  symbol: z.string().max(20),
  name: z.string().max(60),
  group: z.string().max(30),
  price: num.nullable(),
  change_day: num.nullable(),
  change_week: num.nullable(),
  trend: z.enum(["alta", "baixa", "lateral"]),
  weekly_trend: z.enum(["alta", "baixa", "lateral"]),
  daily: z.array(CandleSchema).max(400),
  weekly: z.array(CandleSchema).max(400),
  stormer: z.array(StormerSchema).max(100),
});

export const MarketsSchema = z.object({
  version: z.literal(1),
  generated_ms: ms,
  assets: z.array(MarketAssetSchema).max(200),
});

export const IngestSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("live"), data: LiveSchema, new_events: z.array(EventSchema).max(200) }),
  z.object({ kind: z.literal("daily"), data: DailySchema, new_events: z.array(EventSchema).max(200) }),
  z.object({ kind: z.literal("markets"), data: MarketsSchema, new_events: z.array(EventSchema).max(200) }),
]);

export type AppEvent = z.infer<typeof EventSchema>;
export type Mood = z.infer<typeof MoodSchema>;
export type MarketMood = z.infer<typeof MarketMoodSchema>;
export type MoodItem = z.infer<typeof MoodItemSchema>;
export type Setup = z.infer<typeof SetupSchema>;
export type Pending = z.infer<typeof PendingSchema>;
export type Frame = z.infer<typeof FrameSchema>;
export type Asset = z.infer<typeof AssetSchema>;
export type StormerItem = z.infer<typeof StormerSchema>;
export type MarketAsset = z.infer<typeof MarketAssetSchema>;
export type Markets = z.infer<typeof MarketsSchema>;
export type Book = z.infer<typeof BookSchema>;
export type Live = z.infer<typeof LiveSchema>;
export type Daily = z.infer<typeof DailySchema>;
export type DailyAsset = Daily["assets"][number];
