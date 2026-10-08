import { expect, test } from "vitest";
import { fitTextarea } from "../fit";

test("measuring a field puts back the scroll position the measurement clamped", () => {
  const scroller = { scrollTop: 900 } as HTMLElement;
  const node = {
    style: { height: "60px" },
    // Reading the height forces layout with the field collapsed, which is
    // when WebKit clamps the scroller.
    get scrollHeight() {
      scroller.scrollTop = 120;
      return 480;
    },
  } as unknown as HTMLTextAreaElement;
  fitTextarea(node, scroller);
  expect(node.style.height).toBe("480px");
  expect(scroller.scrollTop).toBe(900);
});

test("a field outside any scroller is simply sized", () => {
  const node = { style: { height: "" }, scrollHeight: 40 } as unknown as HTMLTextAreaElement;
  fitTextarea(node, null);
  expect(node.style.height).toBe("40px");
});
