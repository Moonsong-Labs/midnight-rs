;;; ir-hook.ss: write compiler/analyzed-ir.sexp, the same artifact that
;;; compactc --analyzed-ir writes.
;;;
;;; Usage: compactc --skip-zk --ir-hook ir-hook.ss <src> <target-dir>
;;;
;;; This file imports nothing. The analyzed program is a tree of ordinary
;;; records, so it is read with record-rtd and record-type-field-names. The
;;; language's own unparser, the two procedures and the three version strings
;;; that reflection cannot reach arrive as arguments.
;;;
;;; The vocabulary is the compiler's own: forms keep their language names and
;;; field order (langs.ss is the grammar), identifiers print as the compiler
;;; prints them, and VM instructions print in the notation of the ledger DSL
;;; (midnight-ledger.ss).

(define (fail what x)
  (error 'ir-hook (format "unsupported ~a: ~s" what x)))

;; ---------------------------------------------------------------------
;; Reading records.
;; ---------------------------------------------------------------------

(define (fnames x) (record-type-field-names (record-rtd x)))

(define (field x name)
  (let* ([rtd (record-rtd x)] [fs (record-type-field-names rtd)])
    (let loop ([i 0])
      (cond
        [(= i (vector-length fs)) (fail "field" name)]
        [(eq? (vector-ref fs i) name) ((record-accessor rtd i) x)]
        [else (loop (+ i 1))]))))

;; A nanopass record's type name is <language>:<form>:<nonterminal>.<n>, so
;; the middle component is the form's name in langs.ss.
(define (form x)
  (and (record? x)
       (let* ([n (symbol->string (record-type-name (record-rtd x)))]
              [len (string-length n)])
         (let loop ([i 0] [start #f])
           (cond
             [(= i len) #f]
             [(char=? (string-ref n i) #\:)
              (if start (string->symbol (substring n start i)) (loop (+ i 1) (+ i 1)))]
             [else (loop (+ i 1) start)])))))

;; A VM operand record carries no generated counter, so its name is enough.
(define (vm-form x) (and (record? x) (record-type-name (record-rtd x))))

(define (id? x) (and (record? x) (eq? (record-type-name (record-rtd x)) 'id)))
(define (id->sym i) (string->symbol (format "~a" i)))
(define (id-sym i) (field i 'sym))
(define (id-exported? i) (fxbit-set? (field i 'flags) 0))
(define (id-pure? i) (fxbit-set? (field i 'flags) 2))

;; ---------------------------------------------------------------------
;; Expanded VM instructions, in the ledger DSL's notation.
;; ---------------------------------------------------------------------

(define expand-vm-code #f)
(define make-align #f)

(define (rendered? v) (and (pair? v) (symbol? (car v))))

(define (vm-value->sexp v)
  (cond
    [(or (integer? v) (boolean? v) (string? v)) v]
    [(and (pair? v) (not (rendered? v))) (map vm-value->sexp v)]
    [(rendered? v) v]
    [(null? v) '()]
    [(and (record? v) (form v)) (Expr v)]
    [(record? v)
     (case (vm-form v)
       [(VMstack) '(stack)]
       [(VMvoid) '(void)]
       [(VMalign) `(align ,(field v 'value) ,(field v 'bytes))]
       [(VM+) `(+ ,(vm-value->sexp (field v 'x)) ,(vm-value->sexp (field v 'y)))]
       [(VMvalue->int) `(value->int ,(vm-value->sexp (field v 'x)))]
       [(VMnull) `(null ,(Type (field v 'x)))]
       [(VMmax-sizeof) `(max-sizeof ,(Type (field v 'x)))]
       [(VMleaf-hash) `(leaf-hash ,(vm-value->sexp (field v 'x)))]
       [(VMcoin-commit)
        `(coin-commit ,(vm-value->sexp (field v 'coin))
                      ,(vm-value->sexp (field v 'recipient)))]
       [(VMaligned-concat) `(aligned-concat ,@(map vm-value->sexp (field v 'x*)))]
       [(VMstate-value-null) '(state-value null)]
       [(VMstate-value-cell) `(state-value cell ,(vm-value->sexp (field v 'val)))]
       [(VMstate-value-ADT)
        `(state-value ADT ,(vm-value->sexp (field v 'val)) ,(Type (field v 'type)))]
       [(VMstate-value-array)
        `(state-value array ,@(map vm-value->sexp (field v 'val*)))]
       [(VMstate-value-map)
        `(state-value map
           ,@(map (lambda (k v) `(,(vm-value->sexp k) ,(vm-value->sexp v)))
                  (field v 'key*) (field v 'val*)))]
       [(VMstate-value-merkle-tree)
        `(state-value merkle-tree ,(field v 'nat)
           ,@(map (lambda (k v) `(,(vm-value->sexp k) ,(vm-value->sexp v)))
                  (field v 'key*) (field v 'val*)))]
       [else (fail "VM value" v)])]
    [else (fail "VM value" v)]))

(define (vm-suppressed? v)
  (and (record? v) (not (form v)) (eq? (vm-form v) 'VMsuppress)))

(define (vminstr->sexp vi)
  (let ([args (field vi 'arg*)])
    (if (exists (lambda (a) (vm-suppressed? (cdr a))) args)
        #f
        (let ([rendered
               `(,(string->symbol (field vi 'op))
                 ,@(map (lambda (a) `(,(string->symbol (car a)) ,(vm-value->sexp (cdr a)))) args))])
          (if (and (eq? (car rendered) 'ins) (member '(n (void)) (cdr rendered)))
              #f
              rendered)))))

(define (instructions->sexp vminstr*)
  (fold-right (lambda (vi acc) (let ([s (vminstr->sexp vi)]) (if s (cons s acc) acc)))
              '() vminstr*))

(define (path-value pe)
  (if (record? pe) (Expr (field pe 'expr)) (make-align pe 1)))

(define (expand-ops src path-elt* adt-formal* adt-arg* var-name* expr* vm-code)
  (instructions->sexp
    (expand-vm-code src
      (map path-value path-elt*)
      #f
      (append (map cons adt-formal* adt-arg*)
              (map (lambda (vn ex) (cons (id-sym vn) (Expr ex))) var-name* expr*))
      (field vm-code 'code))))

;; ---------------------------------------------------------------------
;; Types.
;; ---------------------------------------------------------------------

;; The language's own unparser, handed over with the stage.  It prints a type
;; the way this artifact wants it, and it leaves identifier records in place
;; for `pretty-print' to number.
(define unparse #f)

(define (Ftype ftype) (unparse ftype))
(define (Type type) (unparse type))
(define (AdtArg arg) (if (record? arg) (unparse arg) arg))

;; ---------------------------------------------------------------------
;; Expressions.
;; ---------------------------------------------------------------------

(define (TupleArg ta)
  (case (form ta)
    [(single) `(single ,(Expr (field ta 'expr)))]
    [(spread) `(spread ,(field ta 'nat) ,(Expr (field ta 'expr)))]
    [else (fail "tuple argument" ta)]))

(define (MapArg ma)
  `(,(Expr (field ma 'expr)) ,(Type (field ma 'type)) ,(Type (field ma 'type^))))

(define (Fun fun)
  (case (form fun)
    [(fref) `(fref ,(id->sym (field fun 'function-name)))]
    [(circuit) `(circuit ,(map Arg (field fun 'arg*))
                         ,(Type (field fun 'type))
                         ,(Expr (field fun 'expr)))]
    [else (fail "function" fun)]))

(define (Arg arg) (unparse arg))

;; An operation class is a bare symbol, or a record when it carries the coin
;; and recipient argument indices.
(define (OpClass oc)
  (if (record? oc)
      `(,(field oc 'ledger-op-class) ,(field oc 'nat) ,(field oc 'nat^))
      oc))

;; A generic native takes one type argument before its values, one for each
;; distinct type parameter in its declaration.  Scan the result too: a
;; parameter can occur there without appearing in an argument.
(define (native-type-argument* entry arg* type)
  (let ([seen '()])
    (fold-right
      (lambda (maybe-type-param type acc)
        (if (and maybe-type-param (not (memq maybe-type-param seen)))
            (begin
              (set! seen (cons maybe-type-param seen))
              (cons (Type type) acc))
            acc))
      '()
      (field entry 'maybe-type-param*)
      (append (map (lambda (a) (field a 'type)) arg*) (list type)))))

(define (E x) (Expr x))
(define (T x) (Type x))

(define (Expr expr)
  (case (form expr)
    [(quote) `(quote ,(field expr 'datum))]
    [(var-ref) `(var-ref ,(id->sym (field expr 'var-name)))]
    [(default) `(default ,(T (field expr 'type)))]
    [(if) `(if ,(E (field expr 'expr0)) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(elt-ref) `(elt-ref ,(E (field expr 'expr)) ,(field expr 'elt-name) ,(field expr 'nat))]
    [(enum-ref) `(enum-ref ,(T (field expr 'type)) ,(field expr 'elt-name))]
    [(tuple) `(tuple ,@(map TupleArg (field expr 'tuple-arg*)))]
    [(vector) `(vector ,@(map TupleArg (field expr 'tuple-arg*)))]
    [(tuple-ref) `(tuple-ref ,(E (field expr 'expr)) ,(field expr 'kindex))]
    [(tuple-slice) `(tuple-slice ,(T (field expr 'type)) ,(E (field expr 'expr))
                                 ,(field expr 'kindex) ,(field expr 'len))]
    [(vector-ref) `(vector-ref ,(T (field expr 'type)) ,(E (field expr 'expr))
                               ,(E (field expr 'index)))]
    [(vector-slice) `(vector-slice ,(T (field expr 'type)) ,(E (field expr 'expr))
                                   ,(E (field expr 'index)) ,(field expr 'len))]
    [(bytes-ref) `(bytes-ref ,(T (field expr 'type)) ,(E (field expr 'expr))
                             ,(E (field expr 'index)))]
    [(bytes-slice) `(bytes-slice ,(T (field expr 'type)) ,(E (field expr 'expr))
                                 ,(E (field expr 'index)) ,(field expr 'len))]
    [(+) `(+ ,(T (field expr 'type)) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(-) `(- ,(T (field expr 'type)) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(*) `(* ,(T (field expr 'type)) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(<) `(< ,(field expr 'bits) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(<=) `(<= ,(field expr 'bits) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(>) `(> ,(field expr 'bits) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(>=) `(>= ,(field expr 'bits) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(==) `(== ,(T (field expr 'type)) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(!=) `(!= ,(T (field expr 'type)) ,(E (field expr 'expr1)) ,(E (field expr 'expr2)))]
    [(map) `(map ,(field expr 'len) ,(Fun (field expr 'fun))
                 ,@(map MapArg (cons (field expr 'map-arg) (field expr 'map-arg*))))]
    [(fold) `(fold ,(field expr 'len) ,(Fun (field expr 'fun))
                   (,(E (field expr 'expr0)) ,(T (field expr 'type0)))
                   ,@(map MapArg (cons (field expr 'map-arg) (field expr 'map-arg*))))]
    [(call) `(call ,(id->sym (field expr 'function-name)) ,@(map E (field expr 'expr*)))]
    [(new) `(new ,(T (field expr 'type)) ,@(map E (field expr 'expr*)))]
    [(seq) `(seq ,@(map E (field expr 'expr*)) ,(E (field expr 'expr)))]
    [(let*) `(let* ,(map (lambda (l e) `(,(Arg l) ,(E e)))
                         (field expr 'local*) (field expr 'expr*))
               ,(E (field expr 'expr)))]
    [(assert) `(assert ,(E (field expr 'expr)) ,(field expr 'mesg))]
    [(field->bytes) `(field->bytes ,(field expr 'len) ,(Ftype (field expr 'ftype))
                                   ,(E (field expr 'expr)))]
    [(cast-from-bytes) `(cast-from-bytes ,(T (field expr 'type)) ,(field expr 'len)
                                         ,(E (field expr 'expr)))]
    [(vector->bytes) `(vector->bytes ,(field expr 'len) ,(E (field expr 'expr)))]
    [(bytes->vector) `(bytes->vector ,(field expr 'len) ,(E (field expr 'expr)))]
    [(cast-from-enum) `(cast-from-enum ,(T (field expr 'type)) ,(T (field expr 'type^))
                                       ,(E (field expr 'expr)))]
    [(cast-to-enum) `(cast-to-enum ,(T (field expr 'type)) ,(T (field expr 'type^))
                                   ,(E (field expr 'expr)))]
    [(cast-to-field) `(cast-to-field ,(Ftype (field expr 'ftype)) ,(T (field expr 'type))
                                     ,(E (field expr 'expr)))]
    [(cast-from-field) `(cast-from-field ,(field expr 'nat) ,(Ftype (field expr 'ftype))
                                         ,(E (field expr 'expr)))]
    [(safe-cast) `(safe-cast ,(T (field expr 'type)) ,(T (field expr 'type^))
                             ,(E (field expr 'expr)))]
    [(downcast-unsigned) `(downcast-unsigned ,(field expr 'nat2) ,(field expr 'nat1)
                                             ,(E (field expr 'expr)))]
    [(contract-call) `(contract-call ,(field expr 'elt-name)
                                     (,(E (field expr 'expr)) ,(T (field expr 'type)))
                                     ,@(map E (field expr 'expr*)))]
    [(emit)
     (let ([version (field expr 'event-version)]
           [tag (field expr 'event-tag)]
           [payload (E (field expr 'expr))])
       `(emit ,version ,tag ,(field expr 'len) ,payload
          (instructions
            ,@(instructions->sexp
                (expand-vm-code (field expr 'src) #f #f
                  `((emit-version . ,version)
                    (emit-tag . ,tag)
                    (emit-payload . ,payload))
                  (field (field expr 'vm-code) 'code))))))]
    [(public-ledger)
     (let* ([adt-op (field expr 'adt-op)]
            [path-elt* (field expr 'path-elt)]
            [expr* (field expr 'expr*)])
       `(public-ledger ,(id->sym (field expr 'ledger-field-name))
          ,(OpClass (field adt-op 'op-class))
          ,(map (lambda (pe)
                  (if (record? pe)
                      `(,(Type (field pe 'type)) ,(E (field pe 'expr)))
                      pe))
                path-elt*)
          ,(field adt-op 'ledger-op)
          ,(Type (field adt-op 'type))
          (instructions ,@(expand-ops (field expr 'src) path-elt*
                                      (field adt-op 'adt-formal*) (field adt-op 'adt-arg*)
                                      (field adt-op 'var-name*) expr*
                                      (field adt-op 'vm-code)))
          ,@(map E expr*)))]
    [(return) `(return ,(E (field expr 'expr)))]
    [else (fail "expression" expr)]))

;; ---------------------------------------------------------------------
;; Program elements.
;; ---------------------------------------------------------------------

(define (Binding pb)
  `(,(id->sym (field pb 'ledger-field-name))
    ,(field pb 'path-index*)
    (exported ,(id-exported? (field pb 'ledger-field-name)))
    ,(Type (field pb 'type))))

(define (PlArray pl-array)
  `(public-ledger-array
     ,@(map (lambda (elt)
              (if (eq? (form elt) 'public-ledger-array) (PlArray elt) (Binding elt)))
            (field pl-array 'pl-array-elt))))

(define (Pelt pelt proof-id*)
  (case (form pelt)
    [(circuit)
     (let ([fn (field pelt 'function-name)])
       `(circuit ,(id->sym fn)
          (exported ,(id-exported? fn))
          (pure ,(id-pure? fn))
          (proof ,(and (memq fn proof-id*) #t))
          ,(map Arg (field pelt 'arg*))
          ,(Type (field pelt 'type))
          ,(Expr (field pelt 'expr))))]
    [(native)
     (let ([entry (field pelt 'native-entry)])
       `(native ,(id->sym (field pelt 'function-name))
          (entry ,(field entry 'function) ,(field entry 'class))
          (type-arguments ,@(native-type-argument*
                              entry (field pelt 'arg*) (field pelt 'type)))
          ,(map Arg (field pelt 'arg*))
          ,(Type (field pelt 'type))))]
    [(witness)
     `(witness ,(id->sym (field pelt 'function-name))
        ,(map Arg (field pelt 'arg*))
        ,(Type (field pelt 'type)))]
    [(kernel-declaration)
     `(kernel-declaration ,(Binding (field pelt 'public-binding)))]
    [(public-ledger-declaration)
     (let ([c (field pelt 'lconstructor)])
       `(public-ledger-declaration
          ,(PlArray (field pelt 'pl-array))
          (constructor ,(map Arg (field c 'arg*)) ,(Expr (field c 'expr)))))]
    [(export-typedef)
     `(export-typedef ,(field pelt 'type-name) ,(field pelt 'tvar-name*)
        ,(Type (field pelt 'type)))]
    [else (fail "program element" pelt)]))

;; ---------------------------------------------------------------------
;; Entry point.
;; ---------------------------------------------------------------------

(define hook
  (lambda (stage* proof-circuit-name* compiler target-directory)
    (set! expand-vm-code (cdr (assq 'expand-vm-code compiler)))
    (set! make-align (cdr (assq 'align compiler)))
    (set! unparse (caddr (assq 'analyzed stage*)))
    (let* ([ir (cadr (assq 'analyzed stage*))]
           [export-name* (field ir 'export-name*)]
           [name* (field ir 'name*)]
           ;; Collect the id records: id-uniq is assigned on first print, so
           ;; printing one here would renumber the whole artifact.
           [proof-id* (fold-left
                        (lambda (acc en n)
                          (if (memq en proof-circuit-name*) (cons n acc) acc))
                        '() export-name* name*)]
           [sexp `(analyzed-ir
                    (compiler-version ,(cdr (assq 'compiler-version compiler)))
                    (language-version ,(cdr (assq 'language-version compiler)))
                    (runtime-version ,(cdr (assq 'runtime-version compiler)))
                    (exports ,@(map (lambda (en n) `(,en . ,(id->sym n))) export-name* name*))
                    (contract-types ,@(map Type (field ir 'contract-type*)))
                    ,@(map (lambda (pelt) (Pelt pelt proof-id*)) (field ir 'pelt*)))])
      (parameterize ([print-brackets #f])
        (call-with-output-file (string-append target-directory "/compiler/analyzed-ir.sexp")
          (lambda (op) (pretty-print sexp op))
          'replace)))))
