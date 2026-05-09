import { CommandParameter } from "../types";
/**
 * Parses a command parameter as seconds and converts to milliseconds.
 * @param arg - The command parameter representing seconds
 * @returns The value in milliseconds, 0 if value is <= 0, or undefined if parsing fails
 */
export declare const parseSecondsArgument: (arg: CommandParameter | undefined) => number | undefined;
/**
 * Parses the BLOCK option from Redis command arguments (e.g., XREAD, XREADGROUP).
 * @param args - Array of command parameters to search for the BLOCK option
 * @returns The block duration in milliseconds, 0 if duration is <= 0,
 *          null if BLOCK option is not found, or undefined if BLOCK is found but duration is invalid
 */
export declare const parseBlockOption: (args: CommandParameter[]) => number | null | undefined;
