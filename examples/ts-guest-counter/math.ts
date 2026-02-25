// Math utilities module - demonstrates exported functions that can be imported

/**
 * Calculate the sum of an array of numbers.
 */
export function sum(values: number[]): number {
  let total = 0;
  for (const v of values) {
    total = total + v;
  }
  return total;
}

/**
 * Calculate the product of an array of numbers.
 */
export function product(values: number[]): number {
  if (values.length === 0) {
    return 0;
  }
  let result = 1;
  for (const v of values) {
    result = result * v;
  }
  return result;
}

/**
 * Calculate the average of an array of numbers.
 * Returns 0 for empty arrays.
 */
export function average(values: number[]): number {
  if (values.length === 0) {
    return 0;
  }
  return sum(values) / values.length;
}

/**
 * Find the minimum value in an array.
 * Returns Infinity for empty arrays.
 */
export function min(values: number[]): number {
  if (values.length === 0) {
    return Infinity;
  }
  let minVal = values[0];
  for (const v of values) {
    if (v < minVal) {
      minVal = v;
    }
  }
  return minVal;
}

/**
 * Find the maximum value in an array.
 * Returns -Infinity for empty arrays.
 */
export function max(values: number[]): number {
  if (values.length === 0) {
    return -Infinity;
  }
  let maxVal = values[0];
  for (const v of values) {
    if (v > maxVal) {
      maxVal = v;
    }
  }
  return maxVal;
}

/**
 * Clamp a value between min and max bounds.
 */
export function clamp(value: number, minVal: number, maxVal: number): number {
  if (value < minVal) {
    return minVal;
  }
  if (value > maxVal) {
    return maxVal;
  }
  return value;
}

/**
 * Calculate absolute value.
 */
export function abs(value: number): number {
  if (value < 0) {
    return -value;
  }
  return value;
}
