// Multi-file TS guest program demonstrating import resolution.
// This example imports functions from sibling modules and uses them
// to perform data analysis within the Arth TS subset.

// Import from local modules (relative imports)
import { sum, product, average, clamp, abs } from "./math";
import { computeStatistics, filterAbove, normalize, allPositive } from "./data";

// Import from Arth host capabilities
import { Logger, Fields } from "arth:log";
import { arrayOf } from "arth:array";

// Result type for operations that can fail
type Result<T> = { kind: "ok"; value: T } | { kind: "err"; message: string };

// Get logger instance
const log = Logger.get("ts-guest-counter");

/**
 * Main entry point - demonstrates multi-file imports and data processing.
 */
export function main(): number {
  log.info("main.start", "Starting multi-file TS guest demo", {});

  // Create sample data
  const values = arrayOf<number>(10, 25, 15, 30, 5, 20, 35, 8, 22, 18);

  // Test math module functions directly
  const mathResult = testMathFunctions(values);
  if (mathResult.kind === "err") {
    log.error("main.math_failed", mathResult.message, {});
    return 1;
  }

  // Test data module functions (which internally use math module)
  const dataResult = testDataFunctions(values);
  if (dataResult.kind === "err") {
    log.error("main.data_failed", dataResult.message, {});
    return 1;
  }

  // Test chained operations
  const chainResult = testChainedOperations(values);
  if (chainResult.kind === "err") {
    log.error("main.chain_failed", chainResult.message, {});
    return 1;
  }

  log.info("main.complete", "All tests passed successfully", {});
  return 0;
}

/**
 * Test basic math module functions.
 */
export function testMathFunctions(values: number[]): Result<void> {
  // Test sum
  const total = sum(values);
  log.info("math.sum", "Computed sum", Fields.of("total", total));

  // Test product (of smaller array to avoid overflow)
  const smallValues = arrayOf<number>(2, 3, 4);
  const prod = product(smallValues);
  if (prod !== 24) {
    return { kind: "err", message: "product calculation incorrect" };
  }
  log.info("math.product", "Computed product", Fields.of("product", prod));

  // Test average
  const avg = average(values);
  log.info("math.average", "Computed average", Fields.of("average", avg));

  // Test clamp
  const clamped = clamp(150, 0, 100);
  if (clamped !== 100) {
    return { kind: "err", message: "clamp high value failed" };
  }
  const clampedLow = clamp(-50, 0, 100);
  if (clampedLow !== 0) {
    return { kind: "err", message: "clamp low value failed" };
  }
  log.info("math.clamp", "Clamp tests passed", {});

  // Test abs
  const absVal = abs(-42);
  if (absVal !== 42) {
    return { kind: "err", message: "abs calculation incorrect" };
  }
  log.info("math.abs", "Abs test passed", Fields.of("result", absVal));

  return { kind: "ok", value: undefined };
}

/**
 * Test data processing module functions.
 */
export function testDataFunctions(values: number[]): Result<void> {
  // Test computeStatistics (uses sum, average, min, max from math module)
  const stats = computeStatistics(values);
  log.info(
    "data.statistics",
    "Computed statistics",
    Fields.of(
      "count", stats.count,
      "sum", stats.sum,
      "average", stats.average,
      "min", stats.min,
      "max", stats.max,
      "range", stats.range
    )
  );

  // Verify statistics are reasonable
  if (stats.count !== 10) {
    return { kind: "err", message: "statistics count incorrect" };
  }
  if (stats.min > stats.max) {
    return { kind: "err", message: "statistics min > max" };
  }
  if (stats.range !== stats.max - stats.min) {
    return { kind: "err", message: "statistics range incorrect" };
  }

  // Test filterAbove
  const aboveAvg = filterAbove(values, stats.average);
  log.info(
    "data.filter",
    "Filtered above average",
    Fields.of("threshold", stats.average, "count", aboveAvg.length)
  );

  // Test allPositive
  const positiveCheck = allPositive(values);
  if (!positiveCheck) {
    return { kind: "err", message: "allPositive returned false for positive values" };
  }
  log.info("data.allPositive", "All values are positive", {});

  // Test with negative values
  const mixedValues = arrayOf<number>(1, -2, 3, -4, 5);
  const mixedPositive = allPositive(mixedValues);
  if (mixedPositive) {
    return { kind: "err", message: "allPositive should return false for mixed values" };
  }

  return { kind: "ok", value: undefined };
}

/**
 * Test chained operations using functions from multiple modules.
 */
export function testChainedOperations(values: number[]): Result<void> {
  // Chain: normalize -> filterAbove -> compute statistics
  const normalized = normalize(values);
  log.info(
    "chain.normalize",
    "Normalized values to [0,1]",
    Fields.of("count", normalized.length)
  );

  // All normalized values should be in [0, 1]
  for (const v of normalized) {
    if (v < 0 || v > 1) {
      return { kind: "err", message: "normalized value out of range" };
    }
  }

  // Filter normalized values above 0.5 (above median-ish)
  const aboveHalf = filterAbove(normalized, 0.5);
  log.info(
    "chain.filterNormalized",
    "Filtered normalized values above 0.5",
    Fields.of("count", aboveHalf.length)
  );

  // Compute statistics on the filtered set
  const filteredStats = computeStatistics(aboveHalf);
  log.info(
    "chain.filteredStats",
    "Statistics on filtered data",
    Fields.of("count", filteredStats.count, "average", filteredStats.average)
  );

  // Filtered normalized values above 0.5 should all be in (0.5, 1]
  if (filteredStats.count > 0) {
    if (filteredStats.min <= 0.5) {
      return { kind: "err", message: "filtered min should be > 0.5" };
    }
  }

  log.info("chain.complete", "Chained operations test passed", {});
  return { kind: "ok", value: undefined };
}
