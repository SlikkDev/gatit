# Uploading calls to Google Drive

Once connected, Gatit uploads every call to your Google Drive as soon as the recording ends, and again whenever the transcript, summary or title changes. Calls are organized by date and time:

```text
My Drive/
  Gatit/
    2026-10-11/
      14-03 Pricing call with Dana/
        meeting.json
        mic.m4a
        system.m4a
        transcript.json
        summary.json
        meeting.md
```

The folder is named after the call's start time and title, so it is renamed when the summary gives the call a better title or you edit it. Deleting a call in Gatit moves its Drive folder to Drive's trash too.

Gatit asks Google only for the `drive.file` permission: it can see and change the files it created itself and nothing else in your Drive.

## One-time setup (about five minutes)

Gatit is a personal app and its code is public, so it doesn't ship with Google API keys. You create a free OAuth client in your own Google account and paste it into Gatit once.

1. Open the [Google Cloud console](https://console.cloud.google.com/) and sign in with the Google account whose Drive you want to use.
2. Create a project: click the project picker at the top → **New project** → name it `Gatit` → **Create**, then make sure it is selected.
3. Enable the Drive API: open [APIs & Services → Library](https://console.cloud.google.com/apis/library), search for **Google Drive API** and click **Enable**.
4. Set up the consent screen: open [Google Auth Platform → Branding](https://console.cloud.google.com/auth/branding) (older consoles call it **OAuth consent screen**). Fill in the app name `Gatit`, your email as the support email and the developer contact, and save. Under **Audience** pick **External** (or **Internal** if your account is in a Google Workspace organization).
5. Publish the app: on the **Audience** page click **Publish app** and confirm. Gatit only uses a non-sensitive permission, so no verification is needed and nothing else happens. This matters: an app left "in testing" gets sign-ins that expire after 7 days, so Gatit would ask you to sign in again every week.
6. Create the client: open [Google Auth Platform → Clients](https://console.cloud.google.com/auth/clients) (older consoles: **Credentials → Create credentials → OAuth client ID**), click **Create client**, choose **Desktop app** as the type, name it `Gatit` and click **Create**. Copy the **Client ID** and the **Client secret** shown (you can also download them as JSON).
7. In Gatit, open **Settings → Google Drive**, paste the Client ID and Client secret and press **Connect Google Drive**. Your browser opens Google's sign-in: choose the account and allow the single permission. The browser shows "Gatit is connected to Google Drive" and you can close the tab.

Gatit then uploads all the calls it already has, newest first as they come, and every new call from now on.

## Where things are stored

- The client ID, client secret and Google's refresh token are kept in the macOS Keychain (one item named `dev.slikk.gatit` / `google-drive`), never in a file and never in the repository.
- `drive.json` inside each meeting folder records what has been uploaded, so only changed files are sent.
- Settings has a switch to pause automatic uploads; each call also has an **Open in Drive** link and a **Try again** button if an upload failed.

## If something goes wrong

- **"Google Drive needs you to sign in again"**: the sign-in expired or was revoked. Open Settings → Google Drive and press Connect; the client ID and secret are remembered.
- **Two `Gatit` folders in Drive**: Gatit can only see folders it created with the current client. If you connect with a new OAuth client (a new Cloud project), it starts a fresh `Gatit` folder; move the old one's contents in by hand if you like.
- **"Google hasn't verified this app"** in the browser: the app is still in testing (step 5) and your account isn't listed as a test user. Publish it, or add yourself under **Audience → Test users**.
- To revoke Gatit's access entirely, remove it at [myaccount.google.com/permissions](https://myaccount.google.com/permissions) and press **Disconnect Google Drive** in Settings.
