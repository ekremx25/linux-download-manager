# Install the LDM browser extension

Install the Linux Download Manager desktop app first. The installer places the extension files in your Documents folder and registers the native messaging bridge that connects the browser to LDM. Then follow the steps for your browser.

## Google Chrome, Chromium, Brave, or Edge

![Four steps to install the Chrome extension](images/chrome-extension-install-en.svg)

1. Open `chrome://extensions` in Chrome or Chromium. In Edge, open `edge://extensions`; in Brave, open `brave://extensions`.
2. Turn on **Developer mode** in the upper-right corner.
3. Click **Load unpacked**.
4. Select the **`~/Documents/Linux Download Manager Extension` folder**. Choose the folder itself, not its `manifest.json` file. If your file picker does not expand `~`, open your home directory and then `Documents`.
5. Check that **Linux Download Manager Bridge** appears on the extensions page. You can pin it from the toolbar's puzzle icon. Refresh any video pages that were already open.

Chrome uses the unpacked folder; no Chrome ZIP is needed. Keep the folder in place. After updating LDM, click **Reload** on the extension card and refresh the video page.

## Firefox

![Four steps to temporarily install the Firefox extension](images/firefox-extension-install-en.svg)

1. Open `about:debugging` in Firefox.
2. Click **This Firefox** in the left sidebar. Do not stay on the **Setup** page.
3. Click **Load Temporary Add-on**.
4. Select **`~/Documents/Linux Download Manager Firefox Extension.zip`**. Choose the ZIP file itself; there is no need to extract it or choose `manifest.json` inside it.
5. Check that the add-on appears under **Temporary Extensions**, then refresh your video page.

This Firefox installation is temporary: after restarting Firefox, load the same ZIP again. Permanent installation in standard Firefox requires a Mozilla-signed add-on.

If the extension appears but a download does not start, open the LDM desktop app first. After an update, reload the extension and refresh open video pages.
