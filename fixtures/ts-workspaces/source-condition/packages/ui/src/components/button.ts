import { cx } from '../lib/cx.js';

export function button(label: string): string {
  return cx('btn', label);
}
