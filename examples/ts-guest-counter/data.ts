// Data processing module - demonstrates importing from sibling modules
import { sum, average, min, max } from "./math";

/**
 * Statistics result containing computed metrics.
 * Note: Type aliases are kept local since arth-ts doesn't yet support type exports.
 */
type Statistics = {
  count: number;
  sum: number;
  average: number;
  min: number;
  max: number;
  range: number;
};

/**
 * Compute statistics for an array of numbers.
 * Demonstrates using imported functions from math module.
 */
export function computeStatistics(values: number[]): Statistics {
  const count = values.length;
  const sumVal = sum(values);
  const avgVal = average(values);
  const minVal = min(values);
  const maxVal = max(values);
  const range = maxVal - minVal;

  return {
    count: count,
    sum: sumVal,
    average: avgVal,
    min: minVal,
    max: maxVal,
    range: range,
  };
}

/**
 * Filter values that are above a threshold.
 */
export function filterAbove(values: number[], threshold: number): number[] {
  const result: number[] = [];
  for (const v of values) {
    if (v > threshold) {
      result.push(v);
    }
  }
  return result;
}

/**
 * Filter values that are below a threshold.
 */
export function filterBelow(values: number[], threshold: number): number[] {
  const result: number[] = [];
  for (const v of values) {
    if (v < threshold) {
      result.push(v);
    }
  }
  return result;
}

/**
 * Transform each value by applying a scaling factor.
 */
export function scale(values: number[], factor: number): number[] {
  const result: number[] = [];
  for (const v of values) {
    result.push(v * factor);
  }
  return result;
}

/**
 * Normalize values to range [0, 1] based on min/max.
 */
export function normalize(values: number[]): number[] {
  if (values.length === 0) {
    return [];
  }

  const minVal = min(values);
  const maxVal = max(values);
  const range = maxVal - minVal;

  if (range === 0) {
    // All values are the same
    const result: number[] = [];
    for (const v of values) {
      result.push(0.5);
    }
    return result;
  }

  const result: number[] = [];
  for (const v of values) {
    result.push((v - minVal) / range);
  }
  return result;
}

/**
 * Check if all values satisfy a condition (all positive).
 */
export function allPositive(values: number[]): boolean {
  for (const v of values) {
    if (v <= 0) {
      return false;
    }
  }
  return true;
}

/**
 * Check if any value satisfies a condition (any negative).
 */
export function anyNegative(values: number[]): boolean {
  for (const v of values) {
    if (v < 0) {
      return true;
    }
  }
  return false;
}

/**
 * Count how many values match a condition (count zeros).
 */
export function countZeros(values: number[]): number {
  let count = 0;
  for (const v of values) {
    if (v === 0) {
      count = count + 1;
    }
  }
  return count;
}
