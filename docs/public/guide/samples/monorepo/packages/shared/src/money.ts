export type Currency = "USD" | "EUR" | "GBP";

export interface Money {
  readonly cents: number;
  readonly currency: Currency;
}

export class CurrencyMismatchError extends Error {
  constructor(readonly left: Currency, readonly right: Currency) {
    super(`cannot combine ${left} with ${right}`);
  }
}

export function money(cents: number, currency: Currency = "USD"): Money {
  if (!Number.isInteger(cents)) {
    throw new RangeError(`cents must be an integer, got ${cents}`);
  }
  return { cents, currency };
}

export function add(a: Money, b: Money): Money {
  if (a.currency !== b.currency) {
    throw new CurrencyMismatchError(a.currency, b.currency);
  }
  return money(a.cents + b.cents, a.currency);
}

export function sum(items: Money[], currency: Currency = "USD"): Money {
  let total = money(0, currency);
  for (const item of items) {
    total = add(total, item);
  }
  return total;
}

export function percent(amount: Money, rate: number): Money {
  const cents = Math.round(amount.cents * rate);
  return money(cents, amount.currency);
}

export function format(amount: Money): string {
  const value = (amount.cents / 100).toFixed(2);
  return `${amount.currency} ${value}`;
}
