(normalized-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports
   (spend . %spend.107)
   (zerocash_mint . %zerocash_mint.108))
 (contract-types)
 (kernel-declaration (%kernel.163 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%nullifiers.138
       (0)
       (exported #f)
       (Set (tstruct nullifier (bytes (tbytes 32)))))
     (%commitments.135
       (1)
       (exported #f)
       (HistoricMerkleTree
         32
         (tstruct commitment (bytes (tbytes 32)))))
     (%ciphertexts.134
       (2)
       (exported #f)
       (__compact_Cell (topaque "Uint8Array"))))
   (constructor () (tuple)))
 (circuit %merkleTreePathRoot.137 (exported #f) (pure #t) (proof #f)
   ((%path.157
      (tstruct
        MerkleTreePath
        (leaf (tstruct commitment (bytes (tbytes 32))))
        (path
          (tvector
            32
            (tstruct
              MerkleTreePathEntry
              (sibling
                (tstruct MerkleTreeDigest (field (tfield (field-native)))))
              (goes_left (tboolean))))))))
   (tstruct MerkleTreeDigest (field (tfield (field-native))))
   (return
     (new (tstruct
            MerkleTreeDigest
            (field (tfield (field-native))))
          (fold
            32
            (fref %merkleTreePathEntryRoot.158)
            ((call
               %degradeToTransient.149
               (call
                 %persistentHash.152
                 (new (tstruct
                        LeafPreimage
                        (domain_sep (tbytes 6))
                        (data (tstruct commitment (bytes (tbytes 32)))))
                      '#vu8(109 100 110 58 108 104)
                      (elt-ref (var-ref %path.157) leaf 0))))
              (tfield (field-native)))
            ((elt-ref (var-ref %path.157) path 1)
              (tvector
                32
                (tstruct
                  MerkleTreePathEntry
                  (sibling
                    (tstruct
                      MerkleTreeDigest
                      (field (tfield (field-native)))))
                  (goes_left (tboolean))))
              (tstruct
                MerkleTreePathEntry
                (sibling
                  (tstruct
                    MerkleTreeDigest
                    (field (tfield (field-native)))))
                (goes_left (tboolean))))))))
 (circuit %merkleTreePathEntryRoot.158 (exported #f) (pure #t)
   (proof #f)
   ((%recursiveDigest.159 (tfield (field-native)))
     (%entry.160
       (tstruct
         MerkleTreePathEntry
         (sibling
           (tstruct MerkleTreeDigest (field (tfield (field-native)))))
         (goes_left (tboolean)))))
   (tfield (field-native))
   (let* (((%left.161 (tfield (field-native))) (if (elt-ref
                                                     (var-ref %entry.160)
                                                     goes_left
                                                     1)
                                                   (var-ref
                                                     %recursiveDigest.159)
                                                   (elt-ref
                                                     (elt-ref
                                                       (var-ref %entry.160)
                                                       sibling
                                                       0)
                                                     field
                                                     0))))
     (let* (((%right.162 (tfield (field-native))) (if (elt-ref
                                                        (var-ref
                                                          %entry.160)
                                                        goes_left
                                                        1)
                                                      (elt-ref
                                                        (elt-ref
                                                          (var-ref
                                                            %entry.160)
                                                          sibling
                                                          0)
                                                        field
                                                        0)
                                                      (var-ref
                                                        %recursiveDigest.159))))
       (return
         (call
           %transientHash.154
           (tuple
             (single (var-ref %left.161))
             (single (var-ref %right.162))))))))
 (native
   %transientHash.154
   (entry "__compactRuntime.transientHash" circuit)
   ((%value.155 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native
   %persistentHash.118
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.156 (tbytes 32)))
   (tbytes 32))
 (native
   %persistentHash.112
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.151 (tvector 4 (tbytes 32))))
   (tbytes 32))
 (native
   %persistentHash.152
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.153
      (tstruct
        LeafPreimage
        (domain_sep (tbytes 6))
        (data (tstruct commitment (bytes (tbytes 32)))))))
   (tbytes 32))
 (native
   %degradeToTransient.149
   (entry "__compactRuntime.degradeToTransient" circuit)
   ((%x.150 (tbytes 32)))
   (tfield (field-native)))
 (witness
   %private$zk_secret_key.122
   ()
   (tstruct zk_secret_key (bytes (tbytes 32))))
 (witness
   %private$remove_coin.133
   ((%coin.148
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple))
 (witness
   %private$zk_public_key.141
   ()
   (tstruct zk_public_key (bytes (tbytes 32))))
 (witness
   %private$add_coin.143
   ((%coin.146
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple))
 (witness
   %context$path_of.127
   ((%cm.147 (tstruct commitment (bytes (tbytes 32)))))
   (tstruct
     MerkleTreePath
     (leaf (tstruct commitment (bytes (tbytes 32))))
     (path
       (tvector
         32
         (tstruct
           MerkleTreePathEntry
           (sibling
             (tstruct MerkleTreeDigest (field (tfield (field-native)))))
           (goes_left (tboolean)))))))
 (witness
   %context$new_coin_info.129
   ()
   (tstruct
     coin_info
     (nonce (tstruct Nonce (bytes (tbytes 32))))
     (opening (tstruct opening (bytes (tbytes 32))))))
 (witness
   %context$encrypt.132
   ((%pk.144 (topaque "Uint8Array"))
     (%coin.145
       (tstruct
         coin_info
         (nonce (tstruct Nonce (bytes (tbytes 32))))
         (opening (tstruct opening (bytes (tbytes 32)))))))
   (topaque "Uint8Array"))
 (circuit %spend.107 (exported #t) (pure #f) (proof #t)
   ((%dest_public_key.119
      (tstruct
        public_key
        (zk (tstruct zk_public_key (bytes (tbytes 32))))
        (encryption (topaque "Uint8Array"))))
     (%input_coin.120
       (tstruct
         coin_info
         (nonce (tstruct Nonce (bytes (tbytes 32))))
         (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple)
   (seq (let* (((%source_secret_key.121
                  (tstruct zk_secret_key (bytes (tbytes 32)))) (call
                                                                 %private$zk_secret_key.122)))
          (let* (((%old_nullifier.123
                    (tstruct nullifier (bytes (tbytes 32)))) (call
                                                               %derive_nullifier.113
                                                               (var-ref
                                                                 %input_coin.120)
                                                               (var-ref
                                                                 %source_secret_key.121))))
            (seq (assert
                   (if (public-ledger %nullifiers.138 (0) member (tboolean)
                         (instructions (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (push
                             (storage #f)
                             (value
                               (state-value
                                 cell
                                 (var-ref %old_nullifier.123))))
                           (member) (popeq (cached #t) (result (void))))
                         (var-ref %old_nullifier.123))
                       '#f
                       '#t)
                   "spend: Coin already spent")
                 (public-ledger %nullifiers.138 (0) insert (ttuple)
                   (instructions (idx (cached #f) (pushPath #t) (path ((align 0 1))))
                     (push
                       (storage #f)
                       (value
                         (state-value cell (var-ref %old_nullifier.123))))
                     (push (storage #t) (value (state-value null)))
                     (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
                   (var-ref %old_nullifier.123))
                 (let* (((%source_public_key.124
                           (tstruct zk_public_key (bytes (tbytes 32)))) (call
                                                                          %derive_zk_public_key.116
                                                                          (var-ref
                                                                            %source_secret_key.121))))
                   (let* (((%old_commitment.125
                             (tstruct commitment (bytes (tbytes 32)))) (call
                                                                         %commitment_from_coin_info.109
                                                                         (var-ref
                                                                           %input_coin.120)
                                                                         (var-ref
                                                                           %source_public_key.124))))
                     (let* (((%commitment_path.126
                               (tstruct
                                 MerkleTreePath
                                 (leaf
                                   (tstruct
                                     commitment
                                     (bytes (tbytes 32))))
                                 (path
                                   (tvector
                                     32
                                     (tstruct
                                       MerkleTreePathEntry
                                       (sibling
                                         (tstruct
                                           MerkleTreeDigest
                                           (field
                                             (tfield (field-native)))))
                                       (goes_left (tboolean))))))) (call
                                                                     %context$path_of.127
                                                                     (var-ref
                                                                       %old_commitment.125))))
                       (seq (assert
                              (if (let* (((%tmp.136
                                            (tstruct
                                              MerkleTreeDigest
                                              (field
                                                (tfield (field-native))))) (call
                                                                             %merkleTreePathRoot.137
                                                                             (var-ref
                                                                               %commitment_path.126))))
                                    (public-ledger %commitments.135 (1) checkRoot
                                      (tboolean)
                                      (instructions (dup (n 0))
                                        (idx (cached #f)
                                             (pushPath #f)
                                             (path ((align 1 1))))
                                        (idx (cached #f)
                                             (pushPath #f)
                                             (path ((align 2 1))))
                                        (push
                                          (storage #f)
                                          (value
                                            (state-value
                                              cell
                                              (var-ref %tmp.136))))
                                        (member)
                                        (popeq
                                          (cached #t)
                                          (result (void))))
                                      (var-ref %tmp.136)))
                                  (== (tstruct
                                        commitment
                                        (bytes (tbytes 32)))
                                      (var-ref %old_commitment.125)
                                      (elt-ref
                                        (var-ref %commitment_path.126)
                                        leaf
                                        0))
                                  '#f)
                              "spend: Illegal state: merkle path not recognized by public state")
                            (let* (((%fresh_coin_info.128
                                      (tstruct
                                        coin_info
                                        (nonce
                                          (tstruct
                                            Nonce
                                            (bytes (tbytes 32))))
                                        (opening
                                          (tstruct
                                            opening
                                            (bytes (tbytes 32)))))) (call
                                                                      %context$new_coin_info.129)))
                              (let* (((%fresh_commitment.130
                                        (tstruct
                                          commitment
                                          (bytes (tbytes 32)))) (call
                                                                  %commitment_from_coin_info.109
                                                                  (var-ref
                                                                    %fresh_coin_info.128)
                                                                  (elt-ref
                                                                    (var-ref
                                                                      %dest_public_key.119)
                                                                    zk
                                                                    0))))
                                (seq (public-ledger %commitments.135 (1) insert (ttuple)
                                       (instructions
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 1 1))))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 0 1))))
                                         (dup (n 2))
                                         (idx (cached #f)
                                              (pushPath #f)
                                              (path ((align 1 1))))
                                         (push
                                           (storage #t)
                                           (value
                                             (state-value
                                               cell
                                               (leaf-hash
                                                 (var-ref
                                                   %fresh_commitment.130)))))
                                         (ins (cached #f) (n 1))
                                         (ins (cached #t) (n 1))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 1 1))))
                                         (addi (immediate 1))
                                         (ins (cached #t) (n 1))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 2 1))))
                                         (dup (n 2))
                                         (idx (cached #f)
                                              (pushPath #f)
                                              (path ((align 0 1))))
                                         (root)
                                         (push
                                           (storage #t)
                                           (value (state-value null)))
                                         (ins (cached #f) (n 1))
                                         (ins (cached #t) (n 2)))
                                       (var-ref %fresh_commitment.130))
                                     (let* (((%ciphertext.131
                                               (topaque "Uint8Array")) (call
                                                                         %context$encrypt.132
                                                                         (elt-ref
                                                                           (var-ref
                                                                             %dest_public_key.119)
                                                                           encryption
                                                                           1)
                                                                         (var-ref
                                                                           %fresh_coin_info.128))))
                                       (seq (public-ledger %ciphertexts.134 (2) write
                                              (ttuple)
                                              (instructions
                                                (push
                                                  (storage #f)
                                                  (value
                                                    (state-value
                                                      cell
                                                      (align 2 1))))
                                                (push
                                                  (storage #t)
                                                  (value
                                                    (state-value
                                                      cell
                                                      (var-ref
                                                        %ciphertext.131))))
                                                (ins (cached #f) (n 1)))
                                              (var-ref %ciphertext.131))
                                            (call
                                              %private$remove_coin.133
                                              (var-ref
                                                %input_coin.120))))))))))))))
        (return (tuple))))
 (circuit %zerocash_mint.108 (exported #t) (pure #f) (proof #t) ()
   (ttuple)
   (seq (let* (((%coin.139
                  (tstruct
                    coin_info
                    (nonce (tstruct Nonce (bytes (tbytes 32))))
                    (opening (tstruct opening (bytes (tbytes 32)))))) (call
                                                                        %context$new_coin_info.129)))
          (let* (((%pk.140
                    (tstruct zk_public_key (bytes (tbytes 32)))) (call
                                                                   %private$zk_public_key.141)))
            (seq (call %private$add_coin.143 (var-ref %coin.139))
                 (let* (((%cm.142 (tstruct commitment (bytes (tbytes 32)))) (call
                                                                              %commitment_from_coin_info.109
                                                                              (var-ref
                                                                                %coin.139)
                                                                              (var-ref
                                                                                %pk.140))))
                   (public-ledger %commitments.135 (1) insert (ttuple)
                     (instructions (idx (cached #f) (pushPath #t) (path ((align 1 1))))
                       (idx (cached #f) (pushPath #t) (path ((align 0 1))))
                       (dup (n 2))
                       (idx (cached #f) (pushPath #f) (path ((align 1 1))))
                       (push
                         (storage #t)
                         (value
                           (state-value
                             cell
                             (leaf-hash (var-ref %cm.142)))))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 1))
                       (idx (cached #f) (pushPath #t) (path ((align 1 1))))
                       (addi (immediate 1)) (ins (cached #t) (n 1))
                       (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                       (dup (n 2))
                       (idx (cached #f) (pushPath #f) (path ((align 0 1))))
                       (root)
                       (push (storage #t) (value (state-value null)))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 2)))
                     (var-ref %cm.142))))))
        (return (tuple))))
 (circuit %derive_nullifier.113 (exported #f) (pure #t) (proof #f)
   ((%coin.114
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32))))))
     (%sk.115 (tstruct zk_secret_key (bytes (tbytes 32)))))
   (tstruct nullifier (bytes (tbytes 32)))
   (return
     (new (tstruct nullifier (bytes (tbytes 32)))
          (call
            %persistentHash.112
            (tuple
              (single
                '#vu8(108 97 114 101 115 58 122 101 114 111 99 97 115 104
                      58 99 111 109 109 105 116 0 0 0 0 0 0 0 0 0 0 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.114) nonce 0) bytes 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.114) opening 1) bytes 0))
              (single (elt-ref (var-ref %sk.115) bytes 0)))))))
 (circuit %derive_zk_public_key.116 (exported #f) (pure #t)
   (proof #f)
   ((%sk.117 (tstruct zk_secret_key (bytes (tbytes 32)))))
   (tstruct zk_public_key (bytes (tbytes 32)))
   (return
     (new (tstruct zk_public_key (bytes (tbytes 32)))
          (call
            %persistentHash.118
            (elt-ref (var-ref %sk.117) bytes 0)))))
 (circuit %commitment_from_coin_info.109 (exported #f) (pure #t)
   (proof #f)
   ((%coin.110
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32))))))
     (%pk.111 (tstruct zk_public_key (bytes (tbytes 32)))))
   (tstruct commitment (bytes (tbytes 32)))
   (return
     (new (tstruct commitment (bytes (tbytes 32)))
          (call
            %persistentHash.112
            (tuple
              (single
                '#vu8(108 97 114 101 115 58 122 101 114 111 99 97 115 104
                      58 99 111 109 109 105 116 0 0 0 0 0 0 0 0 0 0 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.110) nonce 0) bytes 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.110) opening 1) bytes 0))
              (single (elt-ref (var-ref %pk.111) bytes 0))))))))
