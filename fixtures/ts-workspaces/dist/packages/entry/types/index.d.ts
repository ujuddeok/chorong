export declare class Client {
    readonly url: string;
    constructor(url: string);
    fetch(path: string): string;
}
