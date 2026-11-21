import AppKit
import WebKit

final class Probe: NSObject, WKNavigationDelegate {
    func webView(_ view: WKWebView, didFinish navigation: WKNavigation!) {
        view.evaluateJavaScript("""
        JSON.stringify((() => {
          const rows = [];
          for (const template of ['2000px / 0px','2000px / 1px','0px / 2000px','1px / 2000px'])
            for (const contain of ['none','layout']) for (const padding of ['0','0 1px','1px 0'])
              for (const overflow of ['hidden','auto','scroll']) for (const border of [0,3]) {
                const box = document.createElement('div');
                box.style.cssText = `display:grid;width:0;height:0;grid-template:${template};contain:${contain};padding:${padding};border:${border}px solid;overflow:visible`;
                document.body.append(box);
                const read = () => {
                  const style = getComputedStyle(box);
                  return {tracks:[style.gridTemplateRows,style.gridTemplateColumns],overflow:[style.overflowX,style.overflowY],client:[box.clientWidth,box.clientHeight],scroll:[box.scrollWidth,box.scrollHeight]};
                };
                const before = read(); box.style.overflow = overflow;
                rows.push({template,contain,padding,border,overflow,before,after:read()}); box.remove();
              }
          return {userAgent:navigator.userAgent,rows};
        })())
        """) { result, error in
            guard let json = result as? String else { fputs("probe failed: \(String(describing: error))\n", stderr); exit(1) }
            print(json); exit(0)
        }
    }
}

let application = NSApplication.shared
let view = WKWebView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
let probe = Probe()
view.navigationDelegate = probe
view.loadHTMLString("<!doctype html><body></body>", baseURL: nil)
DispatchQueue.main.asyncAfter(deadline: .now() + 30) { exit(2) }
application.run()
