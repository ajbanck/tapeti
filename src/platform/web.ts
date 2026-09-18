import { Platform, fileToOpened } from './index';

export const webPlatform: Platform = {
  openFiles({ filters, multiple }) {
    return new Promise((resolve) => {
      const input = document.createElement('input');
      input.type = 'file';
      input.multiple = multiple;
      const exts = filters.flatMap((f) => f.extensions).filter((e) => e !== '*');
      if (exts.length) input.accept = exts.map((e) => '.' + e).join(',');
      input.onchange = async () => resolve(await Promise.all(Array.from(input.files ?? []).map(fileToOpened)));
      input.oncancel = () => resolve([]); // fired by modern browsers when the dialog is dismissed
      input.click();
    });
  },

  async saveFile({ suggestedName, bytes, mime }) {
    const blob = new Blob([bytes as BlobPart], { type: mime ?? 'application/octet-stream' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = suggestedName;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 2000);
    return { name: suggestedName };
  },

  readClipboard() {
    return navigator.clipboard.readText();
  },
  writeClipboard(text) {
    return navigator.clipboard.writeText(text);
  },
};
