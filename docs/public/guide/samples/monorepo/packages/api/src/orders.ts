import { Money, money, add, sum, percent, format } from "@shop/shared";

export interface OrderLine {
  sku: string;
  quantity: number;
  unitPrice: Money;
}

export interface Order {
  id: string;
  lines: OrderLine[];
  total: Money;
}

export interface Inventory {
  reserve(sku: string, quantity: number): Promise<boolean>;
  release(sku: string, quantity: number): Promise<void>;
}

const TAX_RATE = 0.2;

export class OrderService {
  private readonly orders = new Map<string, Order>();

  constructor(private readonly inventory: Inventory) {}

  async place(id: string, lines: OrderLine[]): Promise<Order> {
    const reserved: OrderLine[] = [];
    try {
      for (const line of lines) {
        const ok = await this.inventory.reserve(line.sku, line.quantity);
        if (!ok) {
          throw new Error(`out of stock: ${line.sku}`);
        }
        reserved.push(line);
      }
    } catch (err) {
      await this.rollback(reserved);
      throw err;
    }
    const order = { id, lines, total: this.totalFor(lines) };
    this.orders.set(id, order);
    return order;
  }

  totalFor(lines: OrderLine[]): Money {
    const subtotal = sum(lines.map((l) => lineTotal(l)));
    return add(subtotal, percent(subtotal, TAX_RATE));
  }

  receipt(id: string): string {
    const order = this.orders.get(id);
    if (!order) {
      throw new Error(`unknown order ${id}`);
    }
    const rows = order.lines.map((l) => `${l.sku} x${l.quantity}`);
    return [...rows, `Total: ${format(order.total)}`].join("\n");
  }

  private async rollback(lines: OrderLine[]): Promise<void> {
    for (const line of lines) {
      await this.inventory.release(line.sku, line.quantity);
    }
  }
}

export function lineTotal(line: OrderLine): Money {
  return money(line.unitPrice.cents * line.quantity, line.unitPrice.currency);
}
