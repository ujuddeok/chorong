import type { User } from './models/user.js';

export function format(user: User): string {
  return user.name;
}
