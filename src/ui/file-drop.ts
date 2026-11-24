// OS からのファイルのドロップ

import { t as tr } from "../i18n";
import { getCurrentWebview } from "../infra/tauri";
import { currentPlaylist, editablePlaylist, writable } from "../app/selectors";
import { $, state } from "../app/state";
import { clearDropMarks } from "./drag";
import { dropPhotoAlbum, startTransfer } from "./transfer";

// ---------------- OS からのファイルドロップ ----------------

function dropTargetAt(x: number, y: number): { el: HTMLElement; id: string } | null {
  const el = document.elementFromPoint(x, y) as HTMLElement | null;
  const side = el?.closest<HTMLElement>(".side-item[data-pl]");
  if (side && editablePlaylist(side.dataset.pl!)) return { el: side, id: side.dataset.pl! };
  return null;
}

export async function bindFileDrop() {
  const overlay = $("drop-overlay");
  const text = $("drop-text");
  await getCurrentWebview().onDragDropEvent((ev) => {
    const p = ev.payload;
    if (p.type === "leave") {
      overlay.classList.add("hidden");
      clearDropMarks();
      return;
    }
    const pos = "position" in p ? p.position : null;
    const x = pos ? pos.x / devicePixelRatio : 0;
    const y = pos ? pos.y / devicePixelRatio : 0;
    const target = pos ? dropTargetAt(x, y) : null;

    if (p.type === "enter" || p.type === "over") {
      overlay.classList.remove("hidden");
      clearDropMarks();
      if (!state.snap) text.textContent = tr("iPod が接続されていません");
      else if (!writable()) text.textContent = tr("この iPod には書き込めません");
      else if (state.transferring) text.textContent = tr("転送中です");
      else if (target) {
        target.el.classList.add("drop-target");
        text.textContent = tr("転送してプレイリストに追加");
      } else if (dropPhotoAlbum()) {
        text.textContent = tr("転送してアルバム「{a}」に追加", { a: dropPhotoAlbum()!.split("/").join(" / ") });
      } else {
        const pl = currentPlaylist();
        text.textContent = pl && editablePlaylist(pl.id) ? tr("転送して「{a}」に追加", { a: pl.name }) : tr("ドロップして転送");
      }
      overlay.classList.toggle("over-target", !!target);
      return;
    }
    if (p.type === "drop") {
      overlay.classList.add("hidden");
      clearDropMarks();
      if (!state.snap || !p.paths.length) return;
      const pl = currentPlaylist();
      const plId = target?.id ?? (pl && editablePlaylist(pl.id) ? pl.id : null);
      startTransfer(p.paths, plId);
    }
  });
}
