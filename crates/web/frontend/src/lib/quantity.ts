/** Quantities, as `std::parse_quantity` reads them: a number with an optional
 *  decimal or binary suffix, read as a whole signed 64-bit amount. */

const DECIMAL: [string, number][] = [
  ["k", 3],
  ["M", 6],
  ["G", 9],
  ["T", 12],
  ["P", 15],
  ["E", 18],
];

const BINARY: [string, number][] = [
  ["Ki", 10],
  ["Mi", 20],
  ["Gi", 30],
  ["Ti", 40],
  ["Pi", 50],
  ["Ei", 60],
];

const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

const NOT_A_QUANTITY = "Must be a quantity, such as 512Mi or 1.5G";
const NOT_WHOLE = "Must come to a whole number";
const OUT_OF_RANGE = "Must be within the signed 64-bit range";

export type QuantityResult = { amount: bigint } | { problem: string };

/** The amount of the quantity `text`, or why it is not one. */
// spec: ACN#quantities
export function parseQuantity(text: string): QuantityResult {
  const m = /^([+-]?)(?:(\d+)(?:\.(\d*))?|\.(\d+))(.*)$/s.exec(text);
  if (!m) return { problem: NOT_A_QUANTITY };
  const [, sign, whole = "", wholeFraction, onlyFraction, unit] = m;
  const fraction = wholeFraction ?? onlyFraction ?? "";
  let ten = 0n;
  let two = 0n;
  const exponent = /^[eE]([+-]?)(\d+)$/.exec(unit);
  if (exponent) {
    ten = BigInt(exponent[1] + exponent[2]);
  } else if (unit !== "") {
    const binary = BINARY.find(([s]) => s === unit);
    const decimal = DECIMAL.find(([s]) => s === unit);
    if (binary) two = BigInt(binary[1]);
    else if (decimal) ten = BigInt(decimal[1]);
    else return { problem: suffixProblem(text.slice(0, text.length - unit.length), unit) };
  }

  const digits = whole + fraction;
  const significant = digits.replace(/^0+/, "").replace(/0+$/, "");
  if (significant === "") return { amount: 0n };
  ten += BigInt(digits.length - digits.replace(/0+$/, "").length) - BigInt(fraction.length);
  // A non-zero mantissa is at least 1, so a power of ten from 19 up puts the
  // amount past the largest signed 64-bit integer.
  if (ten >= 19n) return { problem: OUT_OF_RANGE };
  // A mantissa with no trailing zero divides by 10^n only if it divides by
  // 5^n, which needs more than n × log10(5) of its digits.
  if (ten < 0n && -ten > 2n * BigInt(significant.length) + 2n) return { problem: NOT_WHOLE };

  const scaled = BigInt(significant) << two;
  let amount: bigint;
  if (ten >= 0n) {
    amount = scaled * 10n ** ten;
  } else {
    const divisor = 10n ** -ten;
    if (scaled % divisor !== 0n) return { problem: NOT_WHOLE };
    amount = scaled / divisor;
  }
  if (sign === "-") amount = -amount;
  if (amount < I64_MIN || amount > I64_MAX) return { problem: OUT_OF_RANGE };
  return { amount };
}

/** Why `unit` is not a suffix, with the quantity it most likely meant. */
function suffixProblem(written: string, unit: string): string {
  const suffix = likelySuffix(unit);
  if (suffix === null) return NOT_A_QUANTITY;
  return unit === "m"
    ? `m is milli; did you mean ${written}${suffix}?`
    : `Did you mean ${written}${suffix}?`;
}

/** The suffix most likely meant by `unit`: a single letter is binary, as in
 *  the convention where `m` means mebibytes, as is a letter followed by `i`,
 *  and a letter followed by `B` is decimal. */
function likelySuffix(unit: string): string | null {
  const lower = unit.toLowerCase();
  if (lower === "b") return "";
  let table: [string, number][];
  let letters: string;
  if (/^[a-z]$/.test(lower)) [table, letters] = [BINARY, "kmgtp"];
  else if (/^[a-z]ib?$/.test(lower)) [table, letters] = [BINARY, "kmgtpe"];
  else if (/^[a-z]b$/.test(lower)) [table, letters] = [DECIMAL, "kmgtpe"];
  else return null;
  const index = letters.indexOf(lower[0]);
  if (index < 0) return null;
  const suffix = table[index][0];
  return suffix === unit ? null : suffix;
}

/** Write `n` as a quantity: with the largest binary suffix it is a multiple of
 *  to two decimal places, otherwise the largest such decimal suffix, otherwise
 *  as a plain integer. */
// spec: ACN#quantities
export function formatQuantity(n: bigint): string {
  const magnitude = n < 0n ? -n : n;
  const sign = n < 0n ? "-" : "";
  const units: [string, bigint][] = [
    ...[...BINARY].reverse().map(([s, p]): [string, bigint] => [s, 1n << BigInt(p)]),
    ...[...DECIMAL].reverse().map(([s, p]): [string, bigint] => [s, 10n ** BigInt(p)]),
  ];
  for (const [suffix, unit] of units) {
    if (magnitude >= unit && (magnitude * 100n) % unit === 0n) {
      const hundredths = (magnitude * 100n) / unit;
      const whole = hundredths / 100n;
      const fraction = hundredths % 100n;
      if (fraction === 0n) return `${sign}${whole}${suffix}`;
      if (fraction % 10n === 0n) return `${sign}${whole}.${fraction / 10n}${suffix}`;
      return `${sign}${whole}.${fraction.toString().padStart(2, "0")}${suffix}`;
    }
  }
  return n.toString();
}
