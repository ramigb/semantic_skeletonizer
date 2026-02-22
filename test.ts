export class GreetingService {
    public message: string;

    constructor(msg: string) {
        this.message = msg;
    }

    public getDefaultGreeting(): string {
        const prefix = "Hello, ";
        const suffix = "!";
        const fullMessage = prefix + this.message + suffix;
        console.log("Generating greeting");
        return fullMessage;
    }
}
