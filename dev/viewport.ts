const controls = document.getElementById("controls") as HTMLFormElement;
const page = document.getElementById("page") as HTMLSelectElement;
const width = document.getElementById("width") as HTMLInputElement;
const height = document.getElementById("height") as HTMLInputElement;
const frame = document.getElementById("frame") as HTMLIFrameElement;
const metrics = document.getElementById("metrics")!;
const hideControls = document.getElementById("hide-controls") as HTMLInputElement;
const inspection = document.getElementById("inspection")!;
const pages: Record<string, string> = {
  company: "./company.html",
  settings: "./settings.html",
  avatar: "./avatar.html",
  props: "./props.html",
  startup: "./startup.html",
};

function measure(): void {
  const r = frame.getBoundingClientRect();
  const inner = frame.contentWindow;
  metrics.textContent =
    `문서 위치 (${Math.round(r.left + window.scrollX)}, ${Math.round(r.top + window.scrollY)}) · ` +
    `현재 화면 위치 (${Math.round(r.left)}, ${Math.round(r.top)}) · ` +
    `iframe ${Math.round(r.width)}×${Math.round(r.height)} CSS px · ` +
    `내부 viewport ${inner?.innerWidth ?? "?"}×${inner?.innerHeight ?? "?"} · DPR ${window.devicePixelRatio}`;
}

function applySize(): void {
  if (!controls.reportValidity()) return;
  frame.width = String(width.valueAsNumber);
  frame.height = String(height.valueAsNumber);
  measure();
}

function loadPage(): void {
  const path = pages[page.value];
  if (!path) return;
  frame.title = `${page.selectedOptions[0].textContent} 검수`;
  frame.src = path;
  inspection.textContent = "";
}

function applyControls(): void {
  const doc = frame.contentDocument;
  if (!doc) return;
  doc.getElementById("viewport-dev-controls")?.remove();
  if (hideControls.checked) {
    const style = doc.createElement("style");
    style.id = "viewport-dev-controls";
    style.textContent = "#controls, .controls, #note { display: none !important; }";
    if (/\/(company|settings)\.html$/.test(frame.contentWindow!.location.pathname)) {
      style.textContent += "#app { height: 100vh; }";
    }
    doc.head.append(style);
  }
  measure();
}

function inspect(): void {
  const doc = frame.contentDocument;
  const inner = frame.contentWindow;
  if (!doc || !inner) return;
  const rect = (el: Element) => {
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  };
  const size = (el: Element) => ({
    rect: rect(el),
    scrollWidth: el.scrollWidth,
    scrollHeight: el.scrollHeight,
    clientWidth: el.clientWidth,
    clientHeight: el.clientHeight,
  });
  const visible = (el: Element) => {
    const style = inner.getComputedStyle(el);
    const r = el.getBoundingClientRect();
    return style.display !== "none" && style.visibility !== "hidden" && r.width > 0 && r.height > 0;
  };
  const elements = (selector: string) =>
    [...doc.querySelectorAll(selector)].map((el) => ({
      id: el.id,
      className: el.className,
      visible: visible(el),
      ...size(el),
    }));
  const fonts = (selector: string) =>
    [...doc.querySelectorAll(selector)].map((el) => {
      const style = inner.getComputedStyle(el);
      return {
        className: el.className,
        font: style.font,
        fontFamily: style.fontFamily,
        fontSize: style.fontSize,
        fontWeight: style.fontWeight,
        lineHeight: style.lineHeight,
        letterSpacing: style.letterSpacing,
      };
    });
  inspection.textContent = JSON.stringify(
    {
      viewport: { width: inner.innerWidth, height: inner.innerHeight, dpr: inner.devicePixelRatio },
      document: size(doc.documentElement),
      app: elements(".ow-app"),
      scale: elements(".sc"),
      loading: elements(".ld"),
      header: elements(".ow-header"),
      popup: elements(".ow-modal, [role='dialog']"),
      visibleLoadingSteps: [...doc.querySelectorAll(".ld__step")].filter(visible).length,
      fonts: {
        status: doc.fonts.status,
        faces: [...doc.fonts].map((font) => ({ family: font.family, status: font.status })),
        scaleTitle: fonts("#sc-title, .sc-title"),
        loading: fonts(".ld"),
        cardBody: fonts(".ow-card .ow-now__text, .ow-card .ow-trail__text, .ow-card .ow-quote"),
      },
      headerButtons: [...doc.querySelectorAll(".ow-header button")].map((el) => ({
        label: el.getAttribute("aria-label") ?? el.getAttribute("title") ?? el.textContent,
        visible: visible(el),
        rect: rect(el),
      })),
    },
    null,
    2,
  );
}

const presets = document.getElementById("presets")!;
for (const [w, h] of [
  [1600, 960],
  [1280, 720],
  [1100, 640],
  [960, 720],
  [800, 600],
  [720, 480],
  [600, 480],
  [480, 360],
]) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = `${w}×${h}`;
  button.addEventListener("click", () => {
    width.value = String(w);
    height.value = String(h);
    applySize();
  });
  presets.append(button);
}
controls.addEventListener("submit", (event) => {
  event.preventDefault();
  applySize();
});
page.addEventListener("change", loadPage);
document.getElementById("reload")!.addEventListener("click", loadPage);
document.getElementById("inspect")!.addEventListener("click", inspect);
hideControls.addEventListener("change", applyControls);
frame.addEventListener("load", applyControls);
window.addEventListener("resize", measure);
window.addEventListener("scroll", measure, { passive: true });
new ResizeObserver(measure).observe(frame);
loadPage();
applySize();
