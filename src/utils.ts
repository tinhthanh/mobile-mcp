import { ActionableError } from "./robot";

export function validatePackageName(packageName: string): void {
	// iOS bundle identifiers (which flow through this same "packageName" parameter
	// for launch/terminate) are allowed to contain hyphens per Apple's CFBundleIdentifier
	// format, e.g. "com.some-company.app".
	if (!/^[a-zA-Z0-9._-]+$/.test(packageName)) {
		throw new ActionableError(`Invalid package name: "${packageName}"`);
	}
}

export function validateLocale(locale: string): void {
	if (!/^[a-zA-Z0-9,\- ]+$/.test(locale)) {
		throw new ActionableError(`Invalid locale: "${locale}"`);
	}
}
