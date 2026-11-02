// The system Xorvio runs on, and what things are called there, so the
// interface says Finder and the menu bar on a Mac, File Explorer and the
// system tray on Windows, and plainer words on Linux, whose desktops differ.
// It needs nothing else, so the player's rules can use it under node too.

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
export const isWindows = typeof navigator !== "undefined" && /Win/.test(navigator.platform || navigator.userAgent);

/** "this Mac", or "this computer" elsewhere. */
export const thisComputer = isMac ? "this Mac" : "this computer";

/** Where Xorvio keeps running once its window is closed. */
export const trayName = isMac ? "the menu bar" : "the system tray";

/** The button that shows a file or folder in the system's file manager. */
export const showInFiles = isMac ? "Show in Finder" : isWindows ? "Show in Explorer" : "Show in Files";

/** Where logins and links are kept. */
export const keychainName = isWindows ? "Windows Credential Manager" : "your system keychain";

/** A network share Xorvio can read folders from. */
export const networkShare = isMac ? "a network share connected in Finder" : isWindows ? "a network share" : "a mounted network share";

/** The default recordings folder, inside the user's Movies or Videos folder. */
export const recordingsFolderName = isMac ? "Movies › Xorvio" : "Videos › Xorvio";
