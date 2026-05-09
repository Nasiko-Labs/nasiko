"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.parseBlockOption = exports.parseSecondsArgument = void 0;
/**
 * Parses a command parameter to a number.
 * @param arg - The command parameter to parse (number, string, or Buffer)
 * @returns The parsed number, or undefined if parsing fails or arg is undefined
 */
const parseNumberArgument = (arg) => {
    if (typeof arg === "number") {
        return arg;
    }
    if (Buffer.isBuffer(arg)) {
        return parseNumberArgument(arg.toString());
    }
    if (typeof arg === "string") {
        const value = Number(arg);
        return Number.isFinite(value) ? value : undefined;
    }
    return undefined;
};
/**
 * Parses a command parameter to a string.
 * @param arg - The command parameter to parse (string or Buffer)
 * @returns The parsed string, or undefined if arg is not a string/Buffer or is undefined
 */
const parseStringArgument = (arg) => {
    if (typeof arg === "string") {
        return arg;
    }
    if (Buffer.isBuffer(arg)) {
        return arg.toString();
    }
    return undefined;
};
/**
 * Parses a command parameter as seconds and converts to milliseconds.
 * @param arg - The command parameter representing seconds
 * @returns The value in milliseconds, 0 if value is <= 0, or undefined if parsing fails
 */
const parseSecondsArgument = (arg) => {
    const value = parseNumberArgument(arg);
    if (value === undefined) {
        return undefined;
    }
    if (value <= 0) {
        return 0;
    }
    return value * 1000;
};
exports.parseSecondsArgument = parseSecondsArgument;
/**
 * Parses the BLOCK option from Redis command arguments (e.g., XREAD, XREADGROUP).
 * @param args - Array of command parameters to search for the BLOCK option
 * @returns The block duration in milliseconds, 0 if duration is <= 0,
 *          null if BLOCK option is not found, or undefined if BLOCK is found but duration is invalid
 */
const parseBlockOption = (args) => {
    for (let i = 0; i < args.length; i++) {
        const token = parseStringArgument(args[i]);
        if (token && token.toLowerCase() === "block") {
            const duration = parseNumberArgument(args[i + 1]);
            if (duration === undefined) {
                return undefined;
            }
            if (duration <= 0) {
                return 0;
            }
            return duration;
        }
    }
    return null;
};
exports.parseBlockOption = parseBlockOption;
