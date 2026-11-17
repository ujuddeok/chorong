interface User {
    name: string;
}
declare function format(user: User): string;
declare function parse(text: string): string;
export { type User, format, parse as read };
