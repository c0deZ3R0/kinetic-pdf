# Kinetic PDF privacy policy

Effective 8 October 2026. Kinetic PDF is published by Corymbia Software.

## The short version

Corymbia Software does not collect your PDFs or personal information through
Kinetic PDF. There are no accounts, analytics, advertising or tracking. Files
stay on your PC during ordinary use. If you enable assistant access, your
chosen assistant can read content and may send it to its AI provider.

## Your files

Kinetic PDF opens and saves the PDF files you choose, on your PC. It never
uploads them anywhere. Highlights, notes and markups are written into the PDF
file itself when you save. An assistant you connect can also read and edit
these files through the commands you authorize. If you share a PDF, whoever
you share it with
can see them.

The name you enter under "Your name on new notes" is saved into each note you
add, as PDFs normally record, so anyone you share the file with can see it.

## What stays on your PC

To work quickly, Kinetic PDF keeps a few files in your Windows user folders:

- a cache of pages that were slow to draw, in `%LOCALAPPDATA%\kinetic-pdf`
  (limited to 1 GB, and removed by deleting that folder);
- the PDF rendering library it uses, in the same folder;
- the name you enter for new notes, in `%APPDATA%\kinetic-pdf\author.txt`;
- your scroll and zoom speeds, and which version of the app last ran (so it
  can show what changed after an update), in
  `%APPDATA%\kinetic-pdf\settings.json`;
- named tools and their settings in
  `%LOCALAPPDATA%\kinetic-pdf\tools.json`.

Kinetic PDF does not upload these local settings or cache files. Assistant
access can expose document information and tool settings to a connected client.

## Network use

For updates, the Microsoft Store version uses Windows rather than connecting
to a Corymbia Software server. It
asks Windows whether the Microsoft Store has a newer version of the app, a few
seconds after it starts and every 12 hours while it stays open, and the Store
installs one only if you choose to. Windows and the Microsoft Store handle that
request, and Microsoft's privacy statement applies to it. Kinetic PDF sends
no document content through the update check.

The version downloaded from GitHub checks
[github.com/c0deZ3R0/kinetic-pdf](https://github.com/c0deZ3R0/kinetic-pdf)
for a newer release a few seconds after it starts and every 12 hours while it
stays open, and downloads the update
only if you choose to install it. That request includes the app's version
number. As with any website visit, GitHub can see your IP address; its own
privacy statement applies to that. No other information is sent.

## Optional assistant access (MCP)

Assistant access starts disabled and must be enabled for each window in
Settings. It creates an authenticated listener on your PC
(`127.0.0.1`). A client with the session credential can read PDF text,
annotations, document paths, layer names, tool settings and application state,
and can control navigation, tools, document edits and saving. Kinetic PDF
sends that information to the connected local client, not directly to an AI
provider. The client may send it to its provider; that client/provider's
privacy policy and settings govern its use. Only connect a client you trust.

The connection credential is generated in memory and copied to the clipboard
only when you choose to copy connection settings. Kinetic PDF does not log or
save it. Your clipboard or client configuration may retain the copy. Disabling
access or closing the window revokes the credential and prevents queued work
from starting. An operation already started may finish. Enabling access again
generates a new credential. Experimental markup proposals require a separate
Settings choice. Demo playback uses the same local application commands and
does not record or upload video; a recorder you choose governs any recording.

## Children

Kinetic PDF does not knowingly collect information from anyone, including
children.

## Changes

If this policy changes, the new version will be posted at this address with a
new effective date.

## Contact

Questions about this policy: open an issue at
[github.com/c0deZ3R0/kinetic-pdf/issues](https://github.com/c0deZ3R0/kinetic-pdf/issues).
