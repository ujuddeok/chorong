import { square, type Shape } from '@fx/maps';
import { add } from '@fx/tscfg';
import { Client } from '@fx/entry';
import { greet, Missing } from '@fx/swap';
import { format, read, type User } from '@fx/bundle';

export function run(): string {
  const s: Shape = square(add(1, 2));
  const c = new Client('https://example.com');
  const u: User = { name: greet('x') };
  return c.fetch(format(u) + read(' y ') + s.area() + Missing);
}
