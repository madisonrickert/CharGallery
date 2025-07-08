export default function devlog(message?: unknown, ...optionalParams: unknown[]) {
    if (process.env.NODE_ENV === "development") {
        console.log(message, ...optionalParams);
    }
}
