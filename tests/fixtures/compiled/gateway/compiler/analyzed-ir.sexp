(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports (claim_deposit . %claim_deposit.14)
   (egress_jobs . %egress_jobs.15) (fee_token . %fee_token.12)
   (fulfill_signing_request . %fulfill_signing_request.13)
   (next_job_id . %next_job_id.10)
   (next_signing_request_id . %next_signing_request_id.11)
   (processed_attestations . %processed_attestations.8)
   (sign . %sign.9) (signing_fee . %signing_fee.6)
   (signing_requests . %signing_requests.7)
   (threshold . %threshold.4)
   (unclaimed_deposits . %unclaimed_deposits.5)
   (validators . %validators.2) (withdraw . %withdraw.3)
   (witness_deposit . %witness_deposit.0)
   (witness_egress . %witness_egress.1))
 (contract-types)
 (kernel-declaration (%kernel.71 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%threshold.4
       (0)
       (exported #t)
       (__compact_Cell (tunsigned 255)))
     (%validators.2
       (1)
       (exported #t)
       (Set (tpoint (curve-jubjub))))
     (%unclaimed_deposits.5
       (2)
       (exported #t)
       (Map (tbytes 32)
            (tstruct
              UnclaimedDeposit
              (amount (tunsigned 340282366920938463463374607431768211455))
              (token_ref (tbytes 32)))))
     (%next_job_id.10 (3) (exported #t) (Counter))
     (%egress_jobs.15
       (4)
       (exported #t)
       (Map (tfield (field-native))
            (tstruct EgressJob
              (id (tunsigned 340282366920938463463374607431768211455))
              (destination (tbytes 32)) (token_ref (tbytes 32))
              (amount (tunsigned 340282366920938463463374607431768211455))
              (status (tenum JobStatus pending completed)))))
     (%processed_attestations.8
       (5)
       (exported #t)
       (Set (tbytes 32)))
     (%signing_fee.6
       (6)
       (exported #t)
       (__compact_Cell (tunsigned 18446744073709551615)))
     (%fee_token.12
       (7)
       (exported #t)
       (__compact_Cell (tbytes 32)))
     (%next_signing_request_id.11 (8) (exported #t) (Counter))
     (%signing_requests.7
       (9)
       (exported #t)
       (Map (tfield (field-native))
            (tstruct SigningRequest (entity_id (tbytes 32))
              (domain_id (tunsigned 255)) (payload (tbytes 32))
              (status (tenum SigningRequestStatus pending fulfilled))
              (signature (tbytes 64))))))
   (constructor () (tuple)))
 (export-typedef
   JobStatus
   ()
   (tenum JobStatus pending completed))
 (export-typedef
   SigningRequestStatus
   ()
   (tenum SigningRequestStatus pending fulfilled))
 (export-typedef
   EgressJob
   ()
   (tstruct EgressJob
     (id (tunsigned 340282366920938463463374607431768211455))
     (destination (tbytes 32)) (token_ref (tbytes 32))
     (amount (tunsigned 340282366920938463463374607431768211455))
     (status (tenum JobStatus pending completed))))
 (export-typedef
   UnclaimedDeposit
   ()
   (tstruct
     UnclaimedDeposit
     (amount (tunsigned 340282366920938463463374607431768211455))
     (token_ref (tbytes 32))))
 (export-typedef
   SigningRequest
   ()
   (tstruct SigningRequest (entity_id (tbytes 32))
     (domain_id (tunsigned 255)) (payload (tbytes 32))
     (status (tenum SigningRequestStatus pending fulfilled))
     (signature (tbytes 64))))
 (export-typedef
   ValidatorSignature
   ()
   (tstruct
     ValidatorSignature
     (pk (tpoint (curve-jubjub)))
     (r (tpoint (curve-jubjub)))
     (s (tfield (field-native)))))
 (circuit %right.67 (exported #f) (pure #t) (proof #f)
   ((%value.68 (tstruct ContractAddress (bytes (tbytes 32)))))
   (tstruct
     Either
     (is_left (tboolean))
     (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
     (right (tstruct ContractAddress (bytes (tbytes 32)))))
   (return
     (new (tstruct
            Either
            (is_left (tboolean))
            (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
            (right (tstruct ContractAddress (bytes (tbytes 32)))))
          '#f
          (default (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (var-ref %value.68))))
 (circuit %receiveShielded.32 (exported #f) (pure #f) (proof #f)
   ((%coin.69
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455)))))
   (ttuple)
   (seq (let* (((%recipient.70
                  (tstruct
                    Either
                    (is_left (tboolean))
                    (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
                    (right (tstruct ContractAddress (bytes (tbytes 32)))))) (call
                                                                              %right.67
                                                                              (public-ledger
                                                                                %kernel.71
                                                                                read
                                                                                ()
                                                                                self
                                                                                (tstruct
                                                                                  ContractAddress
                                                                                  (bytes
                                                                                    (tbytes
                                                                                      32)))
                                                                                (instructions
                                                                                  (dup (n 2))
                                                                                  (idx (cached
                                                                                         #t)
                                                                                       (pushPath
                                                                                         #f)
                                                                                       (path
                                                                                         ((align
                                                                                            0
                                                                                            1))))
                                                                                  (popeq
                                                                                    (cached
                                                                                      #t)
                                                                                    (result
                                                                                      (void))))))))
          (seq (call
                 %createZswapOutput.59
                 (var-ref %coin.69)
                 (var-ref %recipient.70))
               (let* (((%tmp.72 (tbytes 32)) (call
                                               %coinCommitment.62
                                               (var-ref %coin.69)
                                               (var-ref %recipient.70))))
                 (public-ledger %kernel.71 update () claimZswapCoinReceive (ttuple)
                   (instructions (swap (n 0))
                     (idx (cached #t) (pushPath #t) (path ((align 1 1))))
                     (push
                       (storage #f)
                       (value (state-value cell (var-ref %tmp.72))))
                     (push (storage #f) (value (state-value null)))
                     (ins (cached #t) (n 2)) (swap (n 0)))
                   (var-ref %tmp.72)))))
        (return (tuple))))
 (circuit %coinCommitment.62 (exported #f) (pure #t) (proof #f)
   ((%coin.63
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455))))
     (%recipient.64
       (tstruct
         Either
         (is_left (tboolean))
         (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
         (right (tstruct ContractAddress (bytes (tbytes 32)))))))
   (tbytes 32)
   (return
     (call
       %persistentHash.65
       (new (tstruct CoinPreimage (domain_sep (tbytes 21))
              (info
                (tstruct
                  ShieldedCoinInfo
                  (nonce (tbytes 32))
                  (color (tbytes 32))
                  (value
                    (tunsigned 340282366920938463463374607431768211455))))
              (dataType (tboolean)) (data (tbytes 32)))
            '#vu8(109 105 100 110 105 103 104 116 58 122 115 119 97 112
                  45 99 99 91 118 49 93)
            (var-ref %coin.63)
            (elt-ref (var-ref %recipient.64) is_left 0)
            (if (elt-ref (var-ref %recipient.64) is_left 0)
                (elt-ref (elt-ref (var-ref %recipient.64) left 1) bytes 0)
                (elt-ref
                  (elt-ref (var-ref %recipient.64) right 2)
                  bytes
                  0))))))
 (native %persistentHash.65
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments
     (tstruct CoinPreimage (domain_sep (tbytes 21))
       (info
         (tstruct
           ShieldedCoinInfo
           (nonce (tbytes 32))
           (color (tbytes 32))
           (value
             (tunsigned 340282366920938463463374607431768211455))))
       (dataType (tboolean)) (data (tbytes 32))))
   ((%value.66
      (tstruct CoinPreimage (domain_sep (tbytes 21))
        (info
          (tstruct
            ShieldedCoinInfo
            (nonce (tbytes 32))
            (color (tbytes 32))
            (value
              (tunsigned 340282366920938463463374607431768211455))))
        (dataType (tboolean)) (data (tbytes 32)))))
   (tbytes 32))
 (native %persistentHash.22
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tbytes 64)) ((%value.58 (tbytes 64)))
   (tbytes 32))
 (native %createZswapOutput.59
   (entry "__compactRuntime.createZswapOutput" witness)
   (type-arguments)
   ((%coin.60
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455))))
     (%recipient.61
       (tstruct
         Either
         (is_left (tboolean))
         (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
         (right (tstruct ContractAddress (bytes (tbytes 32)))))))
   (ttuple))
 (circuit %count_valid_sig.25 (exported #f) (pure #t) (proof #f)
   ((%sigs.53
      (tvector
        9
        (tstruct
          Maybe
          (is_some (tboolean))
          (value
            (tstruct
              ValidatorSignature
              (pk (tpoint (curve-jubjub)))
              (r (tpoint (curve-jubjub)))
              (s (tfield (field-native)))))))))
   (tunsigned 255)
   (return
     (fold
       9
       (circuit
         ((%a.54 (tunsigned 255))
           (%s.55
             (tstruct
               Maybe
               (is_some (tboolean))
               (value
                 (tstruct
                   ValidatorSignature
                   (pk (tpoint (curve-jubjub)))
                   (r (tpoint (curve-jubjub)))
                   (s (tfield (field-native))))))))
         (tunsigned 255)
         (return
           (downcast-unsigned
             256
             255
             (if (elt-ref (var-ref %s.55) is_some 0)
                 (+ (tunsigned 256)
                    (safe-cast
                      (tunsigned 256)
                      (tunsigned 255)
                      (var-ref %a.54))
                    (safe-cast (tunsigned 256) (tunsigned 1) '1))
                 (safe-cast
                   (tunsigned 256)
                   (tunsigned 255)
                   (var-ref %a.54))))))
       ((safe-cast (tunsigned 255) (tunsigned 0) '0)
         (tunsigned 255))
       ((var-ref %sigs.53)
         (tvector
           9
           (tstruct
             Maybe
             (is_some (tboolean))
             (value
               (tstruct
                 ValidatorSignature
                 (pk (tpoint (curve-jubjub)))
                 (r (tpoint (curve-jubjub)))
                 (s (tfield (field-native)))))))
         (tstruct
           Maybe
           (is_some (tboolean))
           (value
             (tstruct
               ValidatorSignature
               (pk (tpoint (curve-jubjub)))
               (r (tpoint (curve-jubjub)))
               (s (tfield (field-native))))))))))
 (circuit %claim_deposit.14 (exported #t) (pure #f) (proof #t)
   ((%salt.56 (tbytes 32))) (ttuple)
   (seq (let* (((%key.57 (tbytes 32)) (var-ref %salt.56)))
          (seq (assert
                 (public-ledger %unclaimed_deposits.5 read (2) member (tboolean)
                   (instructions (dup (n 0))
                     (idx (cached #f) (pushPath #f) (path ((align 2 1))))
                     (push
                       (storage #f)
                       (value (state-value cell (var-ref %key.57))))
                     (member) (popeq (cached #t) (result (void))))
                   (var-ref %key.57))
                 "claim_deposit: no deposit")
               (public-ledger %unclaimed_deposits.5 remove (2) remove (ttuple)
                 (instructions
                   (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                   (push
                     (storage #f)
                     (value (state-value cell (var-ref %key.57))))
                   (rem (cached #f))
                   (ins (cached #t) (n 1)))
                 (var-ref %key.57))))
        (return (tuple))))
 (circuit %witness_deposit.0 (exported #t) (pure #f) (proof #t)
   ((%sigs.43
      (tvector
        9
        (tstruct
          Maybe
          (is_some (tboolean))
          (value
            (tstruct
              ValidatorSignature
              (pk (tpoint (curve-jubjub)))
              (r (tpoint (curve-jubjub)))
              (s (tfield (field-native))))))))
     (%channel_id.44 (tbytes 32))
     (%amount.41
       (tunsigned 340282366920938463463374607431768211455))
     (%token_ref.42 (tbytes 32)))
   (ttuple)
   (seq (assert
          (let* (((%t.45 (tunsigned 255)) (call
                                            %count_valid_sig.25
                                            (var-ref %sigs.43))))
            (>= 8
                (var-ref %t.45)
                (public-ledger %threshold.4 read (0) read (tunsigned 255)
                  (instructions
                    (dup (n 0))
                    (idx (cached #f) (pushPath #f) (path ((align 0 1))))
                    (popeq (cached #f) (result (void)))))))
          "witness_deposit: threshold")
        (let* (((%tmp.46
                  (tstruct
                    UnclaimedDeposit
                    (amount
                      (tunsigned 340282366920938463463374607431768211455))
                    (token_ref (tbytes 32)))) (new (tstruct
                                                     UnclaimedDeposit
                                                     (amount
                                                       (tunsigned
                                                         340282366920938463463374607431768211455))
                                                     (token_ref
                                                       (tbytes 32)))
                                                   (var-ref %amount.41)
                                                   (var-ref
                                                     %token_ref.42))))
          (public-ledger %unclaimed_deposits.5 update (2) insert (ttuple)
            (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
              (push
                (storage #f)
                (value (state-value cell (var-ref %channel_id.44))))
              (push
                (storage #t)
                (value
                  (state-value
                    ADT
                    (var-ref %tmp.46)
                    (tstruct
                      UnclaimedDeposit
                      (amount
                        (tunsigned
                          340282366920938463463374607431768211455))
                      (token_ref (tbytes 32))))))
              (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
            (var-ref %channel_id.44) (var-ref %tmp.46)))
        (return (tuple))))
 (circuit %witness_egress.1 (exported #t) (pure #f) (proof #t)
   ((%sigs.47
      (tvector
        9
        (tstruct
          Maybe
          (is_some (tboolean))
          (value
            (tstruct
              ValidatorSignature
              (pk (tpoint (curve-jubjub)))
              (r (tpoint (curve-jubjub)))
              (s (tfield (field-native))))))))
     (%job_id.48
       (tunsigned 340282366920938463463374607431768211455)))
   (ttuple)
   (seq (seq (assert
               (let* (((%t.52 (tunsigned 255)) (call
                                                 %count_valid_sig.25
                                                 (var-ref %sigs.47))))
                 (>= 8
                     (var-ref %t.52)
                     (public-ledger %threshold.4 read (0) read (tunsigned 255)
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 0 1))))
                         (popeq (cached #f) (result (void)))))))
               "witness_egress: threshold")
             (let* (((%key.49 (tfield (field-native))) (safe-cast
                                                         (tfield
                                                           (field-native))
                                                         (tunsigned
                                                           340282366920938463463374607431768211455)
                                                         (var-ref
                                                           %job_id.48))))
               (seq (assert
                      (public-ledger %egress_jobs.15 read (4) member (tboolean)
                        (instructions (dup (n 0))
                          (idx (cached #f)
                               (pushPath #f)
                               (path ((align 4 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %key.49))))
                          (member) (popeq (cached #t) (result (void))))
                        (var-ref %key.49))
                      "witness_egress: unknown job")
                    (let* (((%job.50
                              (tstruct EgressJob
                                (id (tunsigned
                                      340282366920938463463374607431768211455))
                                (destination (tbytes 32))
                                (token_ref (tbytes 32))
                                (amount
                                  (tunsigned
                                    340282366920938463463374607431768211455))
                                (status
                                  (tenum JobStatus pending completed)))) (public-ledger
                                                                           %egress_jobs.15
                                                                           read
                                                                           (4)
                                                                           lookup
                                                                           (tstruct
                                                                             EgressJob
                                                                             (id (tunsigned
                                                                                   340282366920938463463374607431768211455))
                                                                             (destination
                                                                               (tbytes
                                                                                 32))
                                                                             (token_ref
                                                                               (tbytes
                                                                                 32))
                                                                             (amount
                                                                               (tunsigned
                                                                                 340282366920938463463374607431768211455))
                                                                             (status
                                                                               (tenum
                                                                                 JobStatus
                                                                                 pending
                                                                                 completed)))
                                                                           (instructions
                                                                             (dup (n 0))
                                                                             (idx (cached
                                                                                    #f)
                                                                                  (pushPath
                                                                                    #f)
                                                                                  (path
                                                                                    ((align
                                                                                       4
                                                                                       1))))
                                                                             (idx (cached
                                                                                    #f)
                                                                                  (pushPath
                                                                                    #f)
                                                                                  (path
                                                                                    ((var-ref
                                                                                       %key.49))))
                                                                             (popeq
                                                                               (cached
                                                                                 #f)
                                                                               (result
                                                                                 (void))))
                                                                           (var-ref
                                                                             %key.49))))
                      (let* (((%tmp.51
                                (tstruct EgressJob
                                  (id (tunsigned
                                        340282366920938463463374607431768211455))
                                  (destination (tbytes 32))
                                  (token_ref (tbytes 32))
                                  (amount
                                    (tunsigned
                                      340282366920938463463374607431768211455))
                                  (status
                                    (tenum JobStatus pending completed)))) (new (tstruct
                                                                                  EgressJob
                                                                                  (id (tunsigned
                                                                                        340282366920938463463374607431768211455))
                                                                                  (destination
                                                                                    (tbytes
                                                                                      32))
                                                                                  (token_ref
                                                                                    (tbytes
                                                                                      32))
                                                                                  (amount
                                                                                    (tunsigned
                                                                                      340282366920938463463374607431768211455))
                                                                                  (status
                                                                                    (tenum
                                                                                      JobStatus
                                                                                      pending
                                                                                      completed)))
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.50)
                                                                                  id
                                                                                  0)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.50)
                                                                                  destination
                                                                                  1)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.50)
                                                                                  token_ref
                                                                                  2)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.50)
                                                                                  amount
                                                                                  3)
                                                                                (enum-ref
                                                                                  (tenum
                                                                                    JobStatus
                                                                                    pending
                                                                                    completed)
                                                                                  completed))))
                        (public-ledger %egress_jobs.15 update (4) insert (ttuple)
                          (instructions
                            (idx (cached #f)
                                 (pushPath #t)
                                 (path ((align 4 1))))
                            (push
                              (storage #f)
                              (value (state-value cell (var-ref %key.49))))
                            (push
                              (storage #t)
                              (value
                                (state-value
                                  ADT
                                  (var-ref %tmp.51)
                                  (tstruct EgressJob
                                    (id (tunsigned
                                          340282366920938463463374607431768211455))
                                    (destination (tbytes 32))
                                    (token_ref (tbytes 32))
                                    (amount
                                      (tunsigned
                                        340282366920938463463374607431768211455))
                                    (status
                                      (tenum
                                        JobStatus
                                        pending
                                        completed))))))
                            (ins (cached #f) (n 1))
                            (ins (cached #t) (n 1)))
                          (var-ref %key.49) (var-ref %tmp.51)))))))
        (return (tuple))))
 (circuit %withdraw.3 (exported #t) (pure #f) (proof #t)
   ((%coin.26
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455))))
     (%destination.27 (tbytes 32)))
   (tunsigned 340282366920938463463374607431768211455)
   (seq (call %receiveShielded.32 (var-ref %coin.26))
        (let* (((%id.28
                  (tunsigned 340282366920938463463374607431768211455)) (safe-cast
                                                                         (tunsigned
                                                                           340282366920938463463374607431768211455)
                                                                         (tunsigned
                                                                           18446744073709551615)
                                                                         (public-ledger
                                                                           %next_job_id.10
                                                                           read
                                                                           (3)
                                                                           read
                                                                           (tunsigned
                                                                             18446744073709551615)
                                                                           (instructions
                                                                             (dup (n 0))
                                                                             (idx (cached
                                                                                    #f)
                                                                                  (pushPath
                                                                                    #f)
                                                                                  (path
                                                                                    ((align
                                                                                       3
                                                                                       1))))
                                                                             (popeq
                                                                               (cached
                                                                                 #t)
                                                                               (result
                                                                                 (void))))))))
          (seq (let* (((%tmp.29 (tunsigned 65535)) (safe-cast
                                                     (tunsigned 65535)
                                                     (tunsigned 1)
                                                     '1)))
                 (public-ledger %next_job_id.10 update (3) increment (ttuple)
                   (instructions
                     (idx (cached #f) (pushPath #t) (path ((align 3 1))))
                     (addi (immediate (value->int (var-ref %tmp.29))))
                     (ins (cached #t) (n 1)))
                   (var-ref %tmp.29)))
               (let* (((%tmp.30 (tfield (field-native))) (safe-cast
                                                           (tfield
                                                             (field-native))
                                                           (tunsigned
                                                             340282366920938463463374607431768211455)
                                                           (var-ref
                                                             %id.28))))
                 (let* (((%tmp.31
                           (tstruct EgressJob
                             (id (tunsigned
                                   340282366920938463463374607431768211455))
                             (destination (tbytes 32))
                             (token_ref (tbytes 32))
                             (amount
                               (tunsigned
                                 340282366920938463463374607431768211455))
                             (status (tenum JobStatus pending completed)))) (new (tstruct
                                                                                   EgressJob
                                                                                   (id (tunsigned
                                                                                         340282366920938463463374607431768211455))
                                                                                   (destination
                                                                                     (tbytes
                                                                                       32))
                                                                                   (token_ref
                                                                                     (tbytes
                                                                                       32))
                                                                                   (amount
                                                                                     (tunsigned
                                                                                       340282366920938463463374607431768211455))
                                                                                   (status
                                                                                     (tenum
                                                                                       JobStatus
                                                                                       pending
                                                                                       completed)))
                                                                                 (var-ref
                                                                                   %id.28)
                                                                                 (var-ref
                                                                                   %destination.27)
                                                                                 (public-ledger
                                                                                   %fee_token.12
                                                                                   read
                                                                                   (7)
                                                                                   read
                                                                                   (tbytes
                                                                                     32)
                                                                                   (instructions
                                                                                     (dup (n 0))
                                                                                     (idx (cached
                                                                                            #f)
                                                                                          (pushPath
                                                                                            #f)
                                                                                          (path
                                                                                            ((align
                                                                                               7
                                                                                               1))))
                                                                                     (popeq
                                                                                       (cached
                                                                                         #f)
                                                                                       (result
                                                                                         (void)))))
                                                                                 (elt-ref
                                                                                   (var-ref
                                                                                     %coin.26)
                                                                                   value
                                                                                   2)
                                                                                 (enum-ref
                                                                                   (tenum
                                                                                     JobStatus
                                                                                     pending
                                                                                     completed)
                                                                                   pending))))
                   (public-ledger %egress_jobs.15 update (4) insert (ttuple)
                     (instructions (idx (cached #f) (pushPath #t) (path ((align 4 1))))
                       (push
                         (storage #f)
                         (value (state-value cell (var-ref %tmp.30))))
                       (push
                         (storage #t)
                         (value
                           (state-value
                             ADT
                             (var-ref %tmp.31)
                             (tstruct EgressJob
                               (id (tunsigned
                                     340282366920938463463374607431768211455))
                               (destination (tbytes 32))
                               (token_ref (tbytes 32))
                               (amount
                                 (tunsigned
                                   340282366920938463463374607431768211455))
                               (status
                                 (tenum JobStatus pending completed))))))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
                     (var-ref %tmp.30) (var-ref %tmp.31))))
               (return (var-ref %id.28))))))
 (circuit %sign.9 (exported #t) (pure #f) (proof #t)
   ((%payload.35 (tbytes 32))
     (%domain_id.36 (tunsigned 255))
     (%salt.33 (tbytes 32))
     (%fee_coin.34
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455)))))
   (tunsigned 340282366920938463463374607431768211455)
   (seq (call %receiveShielded.32 (var-ref %fee_coin.34))
        (let* (((%id.37
                  (tunsigned 340282366920938463463374607431768211455)) (safe-cast
                                                                         (tunsigned
                                                                           340282366920938463463374607431768211455)
                                                                         (tunsigned
                                                                           18446744073709551615)
                                                                         (public-ledger
                                                                           %next_signing_request_id.11
                                                                           read
                                                                           (8)
                                                                           read
                                                                           (tunsigned
                                                                             18446744073709551615)
                                                                           (instructions
                                                                             (dup (n 0))
                                                                             (idx (cached
                                                                                    #f)
                                                                                  (pushPath
                                                                                    #f)
                                                                                  (path
                                                                                    ((align
                                                                                       8
                                                                                       1))))
                                                                             (popeq
                                                                               (cached
                                                                                 #t)
                                                                               (result
                                                                                 (void))))))))
          (seq (let* (((%tmp.38 (tunsigned 65535)) (safe-cast
                                                     (tunsigned 65535)
                                                     (tunsigned 1)
                                                     '1)))
                 (public-ledger %next_signing_request_id.11 update (8) increment
                   (ttuple)
                   (instructions
                     (idx (cached #f) (pushPath #t) (path ((align 8 1))))
                     (addi (immediate (value->int (var-ref %tmp.38))))
                     (ins (cached #t) (n 1)))
                   (var-ref %tmp.38)))
               (let* (((%tmp.39 (tfield (field-native))) (safe-cast
                                                           (tfield
                                                             (field-native))
                                                           (tunsigned
                                                             340282366920938463463374607431768211455)
                                                           (var-ref
                                                             %id.37))))
                 (let* (((%tmp.40
                           (tstruct SigningRequest (entity_id (tbytes 32))
                             (domain_id (tunsigned 255))
                             (payload (tbytes 32))
                             (status
                               (tenum
                                 SigningRequestStatus
                                 pending
                                 fulfilled))
                             (signature (tbytes 64)))) (new (tstruct
                                                              SigningRequest
                                                              (entity_id
                                                                (tbytes
                                                                  32))
                                                              (domain_id
                                                                (tunsigned
                                                                  255))
                                                              (payload
                                                                (tbytes
                                                                  32))
                                                              (status
                                                                (tenum
                                                                  SigningRequestStatus
                                                                  pending
                                                                  fulfilled))
                                                              (signature
                                                                (tbytes
                                                                  64)))
                                                            (var-ref
                                                              %salt.33)
                                                            (var-ref
                                                              %domain_id.36)
                                                            (var-ref
                                                              %payload.35)
                                                            (enum-ref
                                                              (tenum
                                                                SigningRequestStatus
                                                                pending
                                                                fulfilled)
                                                              pending)
                                                            (default
                                                              (tbytes
                                                                64)))))
                   (public-ledger %signing_requests.7 update (9) insert (ttuple)
                     (instructions (idx (cached #f) (pushPath #t) (path ((align 9 1))))
                       (push
                         (storage #f)
                         (value (state-value cell (var-ref %tmp.39))))
                       (push
                         (storage #t)
                         (value
                           (state-value
                             ADT
                             (var-ref %tmp.40)
                             (tstruct SigningRequest (entity_id (tbytes 32))
                               (domain_id (tunsigned 255))
                               (payload (tbytes 32))
                               (status
                                 (tenum
                                   SigningRequestStatus
                                   pending
                                   fulfilled))
                               (signature (tbytes 64))))))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
                     (var-ref %tmp.39) (var-ref %tmp.40))))
               (return (var-ref %id.37))))))
 (circuit %fulfill_signing_request.13 (exported #t) (pure #f)
   (proof #t)
   ((%sigs.17
      (tvector
        9
        (tstruct
          Maybe
          (is_some (tboolean))
          (value
            (tstruct
              ValidatorSignature
              (pk (tpoint (curve-jubjub)))
              (r (tpoint (curve-jubjub)))
              (s (tfield (field-native))))))))
     (%request_id.18
       (tunsigned 340282366920938463463374607431768211455))
     (%signature.16 (tbytes 64)))
   (ttuple)
   (seq (seq (assert
               (let* (((%t.24 (tunsigned 255)) (call
                                                 %count_valid_sig.25
                                                 (var-ref %sigs.17))))
                 (>= 8
                     (var-ref %t.24)
                     (public-ledger %threshold.4 read (0) read (tunsigned 255)
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 0 1))))
                         (popeq (cached #f) (result (void)))))))
               "fulfill: threshold")
             (let* (((%key.19 (tfield (field-native))) (safe-cast
                                                         (tfield
                                                           (field-native))
                                                         (tunsigned
                                                           340282366920938463463374607431768211455)
                                                         (var-ref
                                                           %request_id.18))))
               (seq (assert
                      (public-ledger %signing_requests.7 read (9) member (tboolean)
                        (instructions (dup (n 0))
                          (idx (cached #f)
                               (pushPath #f)
                               (path ((align 9 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %key.19))))
                          (member) (popeq (cached #t) (result (void))))
                        (var-ref %key.19))
                      "fulfill: unknown request")
                    (let* (((%req.20
                              (tstruct SigningRequest (entity_id (tbytes 32))
                                (domain_id (tunsigned 255))
                                (payload (tbytes 32))
                                (status
                                  (tenum
                                    SigningRequestStatus
                                    pending
                                    fulfilled))
                                (signature (tbytes 64)))) (public-ledger
                                                            %signing_requests.7
                                                            read (9) lookup
                                                            (tstruct
                                                              SigningRequest
                                                              (entity_id
                                                                (tbytes
                                                                  32))
                                                              (domain_id
                                                                (tunsigned
                                                                  255))
                                                              (payload
                                                                (tbytes
                                                                  32))
                                                              (status
                                                                (tenum
                                                                  SigningRequestStatus
                                                                  pending
                                                                  fulfilled))
                                                              (signature
                                                                (tbytes
                                                                  64)))
                                                            (instructions
                                                              (dup (n 0))
                                                              (idx (cached
                                                                     #f)
                                                                   (pushPath
                                                                     #f)
                                                                   (path
                                                                     ((align
                                                                        9
                                                                        1))))
                                                              (idx (cached
                                                                     #f)
                                                                   (pushPath
                                                                     #f)
                                                                   (path
                                                                     ((var-ref
                                                                        %key.19))))
                                                              (popeq
                                                                (cached #f)
                                                                (result
                                                                  (void))))
                                                            (var-ref
                                                              %key.19))))
                      (seq (let* (((%tmp.23
                                     (tstruct SigningRequest
                                       (entity_id (tbytes 32))
                                       (domain_id (tunsigned 255))
                                       (payload (tbytes 32))
                                       (status
                                         (tenum
                                           SigningRequestStatus
                                           pending
                                           fulfilled))
                                       (signature (tbytes 64)))) (new (tstruct
                                                                        SigningRequest
                                                                        (entity_id
                                                                          (tbytes
                                                                            32))
                                                                        (domain_id
                                                                          (tunsigned
                                                                            255))
                                                                        (payload
                                                                          (tbytes
                                                                            32))
                                                                        (status
                                                                          (tenum
                                                                            SigningRequestStatus
                                                                            pending
                                                                            fulfilled))
                                                                        (signature
                                                                          (tbytes
                                                                            64)))
                                                                      (elt-ref
                                                                        (var-ref
                                                                          %req.20)
                                                                        entity_id
                                                                        0)
                                                                      (elt-ref
                                                                        (var-ref
                                                                          %req.20)
                                                                        domain_id
                                                                        1)
                                                                      (elt-ref
                                                                        (var-ref
                                                                          %req.20)
                                                                        payload
                                                                        2)
                                                                      (enum-ref
                                                                        (tenum
                                                                          SigningRequestStatus
                                                                          pending
                                                                          fulfilled)
                                                                        fulfilled)
                                                                      (var-ref
                                                                        %signature.16))))
                             (public-ledger %signing_requests.7 update (9) insert
                               (ttuple)
                               (instructions
                                 (idx (cached #f)
                                      (pushPath #t)
                                      (path ((align 9 1))))
                                 (push
                                   (storage #f)
                                   (value
                                     (state-value cell (var-ref %key.19))))
                                 (push
                                   (storage #t)
                                   (value
                                     (state-value
                                       ADT
                                       (var-ref %tmp.23)
                                       (tstruct SigningRequest
                                         (entity_id (tbytes 32))
                                         (domain_id (tunsigned 255))
                                         (payload (tbytes 32))
                                         (status
                                           (tenum
                                             SigningRequestStatus
                                             pending
                                             fulfilled))
                                         (signature (tbytes 64))))))
                                 (ins (cached #f) (n 1))
                                 (ins (cached #t) (n 1)))
                               (var-ref %key.19) (var-ref %tmp.23)))
                           (let* (((%tmp.21 (tbytes 32)) (call
                                                           %persistentHash.22
                                                           (var-ref
                                                             %signature.16))))
                             (public-ledger %processed_attestations.8 update (5) insert
                               (ttuple)
                               (instructions
                                 (idx (cached #f)
                                      (pushPath #t)
                                      (path ((align 5 1))))
                                 (push
                                   (storage #f)
                                   (value
                                     (state-value cell (var-ref %tmp.21))))
                                 (push
                                   (storage #t)
                                   (value (state-value null)))
                                 (ins (cached #f) (n 1))
                                 (ins (cached #t) (n 1)))
                               (var-ref %tmp.21))))))))
        (return (tuple)))))
