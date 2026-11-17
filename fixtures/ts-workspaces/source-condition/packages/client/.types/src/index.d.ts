import { type Session } from '@fx/core';
export declare class Client {
    readonly session: Session;
    constructor(id: string);
    call(path: string): Promise<string>;
}
//# sourceMappingURL=index.d.ts.map