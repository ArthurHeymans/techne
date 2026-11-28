;;; Pure-Scheme tests for Waves 1-3: repeater projection, org-search-headings,
;;; relative date parsing, timestamp shifting, list insertion, table alignment,
;;; subtree releveling, and Markdown/Hugo/HTML export.
(import (techne test))
(require "../org/date.scm")
(require "../org/syntax.scm")
(require "../org/edit.scm")
(require "../org/query.scm")
(require "../org/export.scm")

(define-test "relative date parsing preserves time and repeater"
  (let* ((today (days-from-civil 2026 11 27)) ;; Friday
         (base (parse-org-timestamp "<2026-11-27 Fri 09:30-10:30 +1w -2d>"))
         (ts-plus (org-parse-relative-date "+2d" #:today today #:base-ts base))
         (ts-week (org-parse-relative-date "+1w" #:today today))
         (ts-mon (org-parse-relative-date "mon" #:today today))
         (ts-time (org-parse-relative-date "tomorrow 14:00-15:00" #:today today)))
    (check "plus 2d day" (+ today 2) (org-timestamp-day ts-plus))
    (check "preserves time" 570 (org-timestamp-time ts-plus))
    (check "preserves end time" 630 (org-timestamp-end-time ts-plus))
    (check "preserves repeater" (org-timestamp-repeater base) (org-timestamp-repeater ts-plus))
    (check "preserves warning" (org-timestamp-warning base) (org-timestamp-warning ts-plus))
    (check "plus 1w" (+ today 7) (org-timestamp-day ts-week))
    (check "next mon from fri is +3d" (+ today 3) (org-timestamp-day ts-mon))
    (check "tomorrow with time" (+ today 1) (org-timestamp-day ts-time))
    (check "tomorrow time minutes" 840 (org-timestamp-time ts-time))
    (check "tomorrow end minutes" 900 (org-timestamp-end-time ts-time))))

(define-test "recurring meeting projection and heading search"
  (let* ((doc (string-append
               "* Weekly Sync\n"
               "<2026-11-13 Fri 10:00-11:00 +1w>\n"
               "* TODO Undated backlog task :WORK:\n"
               "* DONE Shipped feature :WORK:\n"))
         (src (make-org-source "/tmp/t.org" doc (org-parse-outline doc)))
         (today (days-from-civil 2026 11 27))
         (entries (org-expand-entries (list src) #:today today #:span 7))
         (sync-days (map org-entry-day entries))
         (all-work (org-search-headings '(tag "WORK") (list src) #:today today #:include-done #t))
         (open-work (org-search-headings '(tag "WORK") (list src) #:today today #:include-done #f)))
    (check "projected weekly meeting on 2026-11-27" #t (if (member today sync-days) #t #f))
    (check "search includes done when asked" 2 (length all-work))
    (check "search skips done by default" 1 (length open-work))))

(define-test "list item continuation and table alignment"
  (let* ((doc "* Notes\n- [ ] first item\n")
         (ol (org-parse-outline doc))
         (res (org-edit-insert-list-item doc ol 12 #:checkbox? #t))
         (new-doc (org-apply-edits doc (car res)))
         (tdoc "* Data\n| Name | Score |\n|---+---|\n| Alice | 100 |\n| Bob | 7 |\n")
         (tol (org-parse-outline tdoc))
         (tedits (org-edit-align-table tdoc tol 10))
         (aligned (org-apply-edits tdoc tedits)))
    (check "inserted sibling checkbox" "* Notes\n- [ ] first item\n- [ ] \n" new-doc)
    (check "aligned table"
           "* Data\n| Name  | Score |\n|-------+-------|\n| Alice | 100   |\n| Bob   | 7     |\n"
           aligned)))

(define-test "subtree releveling and export to md/hugo/html"
  (let* ((sub "* Top\n** Child\nBody\n")
         (releveled (org-edit-relevel-subtree sub 2))
         (doc (string-append
               "#+TITLE: Demo Post\n"
               "#+DATE: 2026-11-27\n"
               "#+FILETAGS: :scheme:org:\n\n"
               "* Intro\n"
               "Hello *world* with [[https://example.com][link]].\n"))
         (ol (org-parse-outline doc))
         (md (org->markdown doc ol))
         (hugo (org->hugo doc ol))
         (html (org->html doc ol)))
    (check "releveled subtree" "*** Top\n**** Child\nBody\n" releveled)
    (check "markdown has bold and link" #t
           (and (string-contains md "**world**")
                (string-contains md "[link](https://example.com)")
                #t))
    (check "hugo has yaml front matter" #t
           (and (string-prefix? "---\ntitle: \"Demo Post\"\n" hugo)
                (string-contains hugo "date: 2026-11-27")
                #t))
    (check "html has strong and anchor" #t
           (and (string-contains html "<strong>world</strong>")
                (string-contains html "<a href=\"https://example.com\">link</a>")
                #t))))
