;;; analyzed-ir-hook.ss: print the analyzed IR (Lloweredemit) as it is,
;;; for use with compactc --run-hook. Writes compiler/analyzed-ir.sexp.
;;;
;;; The vocabulary is the compiler's own: forms keep their language names and
;;; field order (langs.ss is the grammar), identifiers print as the compiler
;;; prints them (%sym.uniq), and VM instructions print in the notation of the
;;; ledger DSL (midnight-ledger.ss). One normalization only: content the
;;; standard unparse drops is made explicit. Each ledger operation and emit
;;; carries its expanded Impact VM instructions, the export table and the
;;; exported/pure/proof flags are printed, and a native carries its runtime
;;; entry. Source objects are dropped, as the unparse drops them.
;;;
;;; Usage: compactc --skip-zk --run-hook analyzed-ir-hook.ss <src> <target-dir>

(import (except (chezscheme) errorf)
        (langs) (nanopass) (vm) (utils) (pass-helpers)
        (compiler-version) (language-version) (runtime-version))

(define (fail what x) (external-errorf "analyzed-ir-hook: unsupported ~a: ~s" what x))

;; An id prints as the compiler prints it; make it a symbol so it reads back.
(define (id->sym i) (string->symbol (format "~a" i)))

;; ----------------------------------------------------------------------
;; Types, in the language's own spellings and field order.
;; ----------------------------------------------------------------------

(define (ftype->sexp ftype)
  (nanopass-case (Lloweredemit Field-Type) ftype
    [(field-native) '(field-native)]
    [(field-base ,ctype) `(field-base ,(ctype->sexp ctype))]
    [(field-scalar ,ctype) `(field-scalar ,(ctype->sexp ctype))]))

(define (ctype->sexp ctype)
  (nanopass-case (Lloweredemit Curve-Type) ctype
    [(curve-jubjub) '(curve-jubjub)]
    [(curve-secp256k1) '(curve-secp256k1)]))

(define (type->sexp type)
  (nanopass-case (Lloweredemit Type) type
    [(tboolean ,src) '(tboolean)]
    [(tfield ,src ,ftype) `(tfield ,(ftype->sexp ftype))]
    [(tunsigned ,src ,nat) `(tunsigned ,nat)]
    [(tpoint ,src ,ctype) `(tpoint ,(ctype->sexp ctype))]
    [(tbytes ,src ,len) `(tbytes ,len)]
    [(topaque ,src ,opaque-type) `(topaque ,opaque-type)]
    [(tvector ,src ,len ,type) `(tvector ,len ,(type->sexp type))]
    [(ttuple ,src ,type* ...) `(ttuple ,@(map type->sexp type*))]
    [(tstruct ,src ,struct-name (,elt-name* ,type*) ...)
     `(tstruct ,struct-name ,@(map (lambda (n t) `(,n ,(type->sexp t))) elt-name* type*))]
    [(tenum ,src ,enum-name ,elt-name ,elt-name* ...)
     `(tenum ,enum-name ,elt-name ,@elt-name*)]
    [(talias ,src ,nominal? ,type-name ,type)
     `(talias ,nominal? ,type-name ,(type->sexp type))]
    [(tcontract ,src ,contract-name (,elt-name* ,pure-dcl* (,type** ...) ,type*) ...)
     `(tcontract ,contract-name
        ,@(map (lambda (n p ts t) `(,n ,p ,(map type->sexp ts) ,(type->sexp t)))
               elt-name* pure-dcl* type** type*))]
    [(tadt ,src ,adt-name ([,adt-formal* ,adt-arg*] ...) ,vm-expr (,adt-op* ...) (,adt-rt-op* ...))
     `(,adt-name ,@(map adt-arg->sexp adt-arg*))]
    [,tvar-name tvar-name]
    [(tunknown) '(tunknown)]
    [else (fail "type" (unparse-Lloweredemit type))]))

(define (adt-arg->sexp arg)
  (nanopass-case (Lloweredemit Public-Ledger-ADT-Arg) arg
    [,type (type->sexp type)]
    [,nat nat]))

;; ----------------------------------------------------------------------
;; Expanded VM instructions, in the ledger DSL's notation.
;; ----------------------------------------------------------------------

(define (rendered? v) (and (pair? v) (symbol? (car v))))

(define (vm-value->sexp v)
  (cond
    [(or (integer? v) (boolean? v) (string? v)) v]
    [(and (pair? v) (not (rendered? v))) (map vm-value->sexp v)]  ; a path list
    [(rendered? v) v]                                            ; a rendered expr
    [(null? v) '()]
    [(VMop? v)
     (VMop-case v
       [(VMstack) '(stack)]
       [(VMvoid) '(void)]
       [(VMalign value bytes) `(align ,value ,bytes)]
       [(VMvalue->int x) `(value->int ,(vm-value->sexp x))]
       [(VMnull x) `(null ,(vm-value->sexp x))]
       [(VMmax-sizeof x) `(max-sizeof ,(vm-value->sexp x))]
       [(VMleaf-hash x) `(leaf-hash ,(vm-value->sexp x))]
       [(VMcoin-commit coin recipient)
        `(coin-commit ,(vm-value->sexp coin) ,(vm-value->sexp recipient))]
       [(VMaligned-concat x*) `(aligned-concat ,@(map vm-value->sexp x*))]
       [(VMstate-value-null) '(state-value null)]
       [(VMstate-value-cell val) `(state-value cell ,(vm-value->sexp val))]
       [(VMstate-value-ADT val type) `(state-value ADT ,(vm-value->sexp val))]
       [(VMstate-value-array val*) `(state-value array ,@(map vm-value->sexp val*))]
       [(VMstate-value-map key* val*)
        `(state-value map ,@(map (lambda (k v) `(,(vm-value->sexp k) ,(vm-value->sexp v))) key* val*))]
       [(VMstate-value-merkle-tree nat key* val*)
        `(state-value merkle-tree ,nat
           ,@(map (lambda (k v) `(,(vm-value->sexp k) ,(vm-value->sexp v))) key* val*))]
       [else (fail "VM value" v)])]
    [else (expr->sexp v)]))

(define (vm-suppressed? v)
  (and (VMop? v) (VMop-case v [(VMsuppress) #t] [else #f])))

;; #f when the instruction is suppressed away (suppress-null/suppress-zero).
(define (vminstr->sexp vi)
  (let ([args (vminstr-arg* vi)])
    (if (ormap (lambda (a) (vm-suppressed? (cdr a))) args)
        #f
        (let ([rendered
               `(,(string->symbol (vminstr-op vi))
                 ,@(map (lambda (a) `(,(string->symbol (car a)) ,(vm-value->sexp (cdr a)))) args))])
          ;; An ins whose count folded to (void) inserts nothing.
          (if (and (eq? (car rendered) 'ins)
                   (member '(n (void)) (cdr rendered)))
              #f
              rendered)))))

(define (expand-ops src path-elt* adt-formal* adt-arg* var-name* expr* vm-code)
  (fold-right
    (lambda (vi acc) (let ([s (vminstr->sexp vi)]) (if s (cons s acc) acc)))
    '()
    (expand-vm-code src
      (map (lambda (pe)
             (nanopass-case (Lloweredemit Path-Element) pe
               [,path-index (VMalign path-index 1)]
               [(,src ,type ,expr) (expr->sexp expr)]))
           path-elt*)
      #f
      (append (map cons adt-formal* adt-arg*)
              (map (lambda (vn ex) (cons (id-sym vn) (expr->sexp ex))) var-name* expr*))
      (vm-code-code vm-code))))

;; ----------------------------------------------------------------------
;; Expressions, in the language's own spellings and field order.
;; ----------------------------------------------------------------------

(define (tuple-arg->sexp ta)
  (nanopass-case (Lloweredemit Tuple-Argument) ta
    [(single ,src ,expr) `(single ,(expr->sexp expr))]
    [(spread ,src ,nat ,expr) `(spread ,nat ,(expr->sexp expr))]))

(define (map-arg->sexp ma)
  (nanopass-case (Lloweredemit Map-Argument) ma
    [(,expr ,type ,type^) `(,(expr->sexp expr) ,(type->sexp type) ,(type->sexp type^))]))

(define (fun->sexp fun)
  (nanopass-case (Lloweredemit Function) fun
    [(fref ,src ,function-name) `(fref ,(id->sym function-name))]
    [(circuit ,src (,arg* ...) ,type ,expr)
     `(circuit ,(map argument->sexp arg*) ,(type->sexp type) ,(expr->sexp expr))]))

(define (argument->sexp arg)
  (nanopass-case (Lloweredemit Argument) arg
    [(,var-name ,type) `(,(id->sym var-name) ,(type->sexp type))]))

(define (expr->sexp expr)
  (nanopass-case (Lloweredemit Expression) expr
    [(quote ,src ,datum) `(quote ,datum)]
    [(var-ref ,src ,var-name) `(var-ref ,(id->sym var-name))]
    [(default ,src ,type) `(default ,(type->sexp type))]
    [(if ,src ,expr0 ,expr1 ,expr2)
     `(if ,(expr->sexp expr0) ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(elt-ref ,src ,expr ,elt-name ,nat) `(elt-ref ,(expr->sexp expr) ,elt-name ,nat)]
    [(enum-ref ,src ,type ,elt-name) `(enum-ref ,(type->sexp type) ,elt-name)]
    [(tuple ,src ,tuple-arg* ...) `(tuple ,@(map tuple-arg->sexp tuple-arg*))]
    [(vector ,src ,tuple-arg* ...) `(vector ,@(map tuple-arg->sexp tuple-arg*))]
    [(tuple-ref ,src ,expr ,kindex) `(tuple-ref ,(expr->sexp expr) ,kindex)]
    [(tuple-slice ,src ,type ,expr ,kindex ,len)
     `(tuple-slice ,(type->sexp type) ,(expr->sexp expr) ,kindex ,len)]
    [(vector-ref ,src ,type ,expr ,index)
     `(vector-ref ,(type->sexp type) ,(expr->sexp expr) ,(expr->sexp index))]
    [(vector-slice ,src ,type ,expr ,index ,len)
     `(vector-slice ,(type->sexp type) ,(expr->sexp expr) ,(expr->sexp index) ,len)]
    [(bytes-ref ,src ,type ,expr ,index)
     `(bytes-ref ,(type->sexp type) ,(expr->sexp expr) ,(expr->sexp index))]
    [(bytes-slice ,src ,type ,expr ,index ,len)
     `(bytes-slice ,(type->sexp type) ,(expr->sexp expr) ,(expr->sexp index) ,len)]
    [(+ ,src ,type ,expr1 ,expr2) `(+ ,(type->sexp type) ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(- ,src ,type ,expr1 ,expr2) `(- ,(type->sexp type) ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(* ,src ,type ,expr1 ,expr2) `(* ,(type->sexp type) ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(< ,src ,bits ,expr1 ,expr2) `(< ,bits ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(<= ,src ,bits ,expr1 ,expr2) `(<= ,bits ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(> ,src ,bits ,expr1 ,expr2) `(> ,bits ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(>= ,src ,bits ,expr1 ,expr2) `(>= ,bits ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(== ,src ,type ,expr1 ,expr2) `(== ,(type->sexp type) ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(!= ,src ,type ,expr1 ,expr2) `(!= ,(type->sexp type) ,(expr->sexp expr1) ,(expr->sexp expr2))]
    [(map ,src ,len ,fun ,map-arg ,map-arg* ...)
     `(map ,len ,(fun->sexp fun) ,@(map map-arg->sexp (cons map-arg map-arg*)))]
    [(fold ,src ,len ,fun (,expr0 ,type0) ,map-arg ,map-arg* ...)
     `(fold ,len ,(fun->sexp fun) (,(expr->sexp expr0) ,(type->sexp type0))
        ,@(map map-arg->sexp (cons map-arg map-arg*)))]
    [(call ,src ,function-name ,expr* ...)
     `(call ,(id->sym function-name) ,@(map expr->sexp expr*))]
    [(new ,src ,type ,expr* ...)
     `(new ,(type->sexp type) ,@(map expr->sexp expr*))]
    [(seq ,src ,expr* ... ,expr)
     `(seq ,@(map expr->sexp expr*) ,(expr->sexp expr))]
    [(let* ,src ([,local* ,expr*] ...) ,expr)
     `(let* ,(map (lambda (l e) `(,(argument->sexp l) ,(expr->sexp e))) local* expr*)
        ,(expr->sexp expr))]
    [(assert ,src ,expr ,mesg) `(assert ,(expr->sexp expr) ,mesg)]
    [(field->bytes ,src ,len ,ftype ,expr)
     `(field->bytes ,len ,(ftype->sexp ftype) ,(expr->sexp expr))]
    [(cast-from-bytes ,src ,type ,len ,expr)
     `(cast-from-bytes ,(type->sexp type) ,len ,(expr->sexp expr))]
    [(vector->bytes ,src ,len ,expr) `(vector->bytes ,len ,(expr->sexp expr))]
    [(bytes->vector ,src ,len ,expr) `(bytes->vector ,len ,(expr->sexp expr))]
    [(cast-from-enum ,src ,type ,type^ ,expr)
     `(cast-from-enum ,(type->sexp type) ,(type->sexp type^) ,(expr->sexp expr))]
    [(cast-to-enum ,src ,type ,type^ ,expr)
     `(cast-to-enum ,(type->sexp type) ,(type->sexp type^) ,(expr->sexp expr))]
    [(cast-to-field ,src ,ftype ,type ,expr)
     `(cast-to-field ,(ftype->sexp ftype) ,(type->sexp type) ,(expr->sexp expr))]
    [(cast-from-field ,src ,nat ,ftype ,expr)
     `(cast-from-field ,nat ,(ftype->sexp ftype) ,(expr->sexp expr))]
    [(safe-cast ,src ,type ,type^ ,expr)
     `(safe-cast ,(type->sexp type) ,(type->sexp type^) ,(expr->sexp expr))]
    [(downcast-unsigned ,src ,nat2 ,nat1 ,expr)
     `(downcast-unsigned ,nat2 ,nat1 ,(expr->sexp expr))]
    [(contract-call ,src ,elt-name (,expr ,type) ,expr* ...)
     `(contract-call ,elt-name (,(expr->sexp expr) ,(type->sexp type))
        ,@(map expr->sexp expr*))]
    [(emit ,src ,event-version ,event-tag ,len ,expr ,vm-code)
     `(emit ,event-version ,event-tag ,len ,(expr->sexp expr)
        (instructions
          ,@(fold-right
              (lambda (vi acc) (let ([s (vminstr->sexp vi)]) (if s (cons s acc) acc)))
              '()
              (expand-vm-code src #f #f
                `((emit-version . ,event-version)
                  (emit-tag . ,event-tag)
                  (emit-payload . ,(expr->sexp expr)))
                (vm-code-code vm-code)))))]
    [(public-ledger ,src ,ledger-field-name ,sugar (,path-elt* ...) ,src^ ,adt-op ,expr* ...)
     (nanopass-case (Lloweredemit ADT-Op) adt-op
       [(,ledger-op ,op-class (,adt-name (,adt-formal* ,adt-arg*) ...) ((,var-name* ,type*) ...) ,type ,vm-code)
        `(public-ledger ,(id->sym ledger-field-name)
           ,(map (lambda (pe)
                   (nanopass-case (Lloweredemit Path-Element) pe
                     [,path-index path-index]
                     [(,src ,type ,expr) `(,(type->sexp type) ,(expr->sexp expr))]))
                 path-elt*)
           ,ledger-op
           ,(type->sexp type)
           (instructions ,@(expand-ops src path-elt* adt-formal* adt-arg* var-name* expr* vm-code))
           ,@(map expr->sexp expr*))])]
    [(return ,src ,expr) `(return ,(expr->sexp expr))]
    [else (fail "expression" (unparse-Lloweredemit expr))]))

;; ----------------------------------------------------------------------
;; Program elements.
;; ----------------------------------------------------------------------

(define proof-circuit-name* '())

(define (pelt->sexp pelt)
  (nanopass-case (Lloweredemit Program-Element) pelt
    [(circuit ,src ,function-name (,arg* ...) ,type ,expr)
     `(circuit ,(id->sym function-name)
        (exported ,(id-exported? function-name))
        (pure ,(id-pure? function-name))
        (proof ,(and (memq (id-sym function-name) proof-circuit-name*) #t))
        ,(map argument->sexp arg*)
        ,(type->sexp type)
        ,(expr->sexp expr))]
    [(native ,src ,function-name ,native-entry (,arg* ...) ,type)
     `(native ,(id->sym function-name)
        (entry ,(native-entry-function native-entry) ,(native-entry-class native-entry))
        ,(map argument->sexp arg*)
        ,(type->sexp type))]
    [(witness ,src ,function-name (,arg* ...) ,type)
     `(witness ,(id->sym function-name) ,(map argument->sexp arg*) ,(type->sexp type))]
    [(kernel-declaration ,public-binding)
     `(kernel-declaration ,(binding->sexp public-binding))]
    [(public-ledger-declaration ,pl-array ,lconstructor)
     `(public-ledger-declaration
        ,(pl-array->sexp pl-array)
        ,(nanopass-case (Lloweredemit Ledger-Constructor) lconstructor
           [(constructor ,src (,arg* ...) ,expr)
            `(constructor ,(map argument->sexp arg*) ,(expr->sexp expr))]))]
    [(export-typedef ,src ,type-name (,tvar-name* ...) ,type)
     `(export-typedef ,type-name ,tvar-name* ,(type->sexp type))]
    [else (fail "program element" (unparse-Lloweredemit pelt))]))

(define (binding->sexp pb)
  (nanopass-case (Lloweredemit Public-Ledger-Binding) pb
    [(,src ,ledger-field-name (,path-index* ...) ,type)
     `(,(id->sym ledger-field-name)
       ,path-index*
       (exported ,(id-exported? ledger-field-name))
       ,(type->sexp type))]))

(define (pl-array->sexp pl-array)
  (nanopass-case (Lloweredemit Public-Ledger-Array) pl-array
    [(public-ledger-array ,pl-array-elt* ...)
     `(public-ledger-array
        ,@(map (lambda (elt)
                 (nanopass-case (Lloweredemit Public-Ledger-Array-Element) elt
                   [,pl-array (pl-array->sexp pl-array)]
                   [,public-binding (binding->sexp public-binding)]))
               pl-array-elt*))]))

(define (program->sexp ir)
  (nanopass-case (Lloweredemit Program) ir
    [(program ,src (,contract-type* ...) ((,export-name* ,name*) ...) ,pelt* ...)
     `(analyzed-ir
        (compiler-version ,compiler-version-string)
        (language-version ,language-version-string)
        (runtime-version ,runtime-version-string)
        (exports ,@(map (lambda (en n) `(,en . ,(id->sym n))) export-name* name*))
        (contract-types ,@(map (lambda (ct)
                                 (nanopass-case (Lloweredemit Contract-Type) ct
                                   [(tcontract ,src ,contract-name (,elt-name* ,pure-dcl* (,type** ...) ,type*) ...)
                                    (type->sexp ct)]))
                               contract-type*))
        ,@(map pelt->sexp pelt*))]))

;; ----------------------------------------------------------------------
;; The hook.
;; ----------------------------------------------------------------------

(define hook
  (lambda (pass-name unparse formats . x*)
    (when (eq? pass-name 'desugar-contract-calls)
      (set! proof-circuit-name*
        (nanopass-case (Lflattened Program) (car x*)
          [(program ,src ((,export-name* ,name*) ...) ,pelt* ...) export-name*])))
    (when (eq? pass-name 'save-contract-info)
      (with-output-to-file (format "~a/compiler/analyzed-ir.sexp" (target-directory))
        (lambda ()
          (parameterize ([print-brackets #f])
            (pretty-print (program->sexp (car x*)))))
        'replace))))
