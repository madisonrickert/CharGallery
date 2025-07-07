import * as Leap from "leapjs";
import * as THREE from 'three';
import { mapLeapToThreePosition } from './util';

export class HandMesh extends THREE.Object3D {
    static boneGeometry = new THREE.SphereGeometry(10, 3, 3);
    static boneMeshMaterial = new THREE.MeshBasicMaterial({
        color: 0xadd6b6,
        wireframeLinewidth: 5,
        wireframe: true,
    });
    static boneLineMaterial = new THREE.LineBasicMaterial({
        color: 0xadd6b6,
        linewidth: 5,
    });

    private bones: { [id: string]: THREE.Mesh } = {};
    private fingers: { [id: string]: THREE.Line } = {};

    constructor() {
        super();
        this.name = 'Hand';
    }

    private addFinger(fingerType: number): THREE.Line {
        if (!this.fingers["finger" + fingerType]) {
            const fingerLine = new THREE.Line(new THREE.BufferGeometry(), HandMesh.boneLineMaterial);
            this.fingers["finger" + fingerType] = fingerLine;
            this.add(fingerLine);
        }
        return this.fingers["finger" + fingerType];
    }

    private addBone(fingerType: number, boneType: number): THREE.Mesh {
        const id = `${fingerType},${boneType}`;
        if (!this.bones[id]) {
            const boneMesh = new THREE.Mesh(HandMesh.boneGeometry, HandMesh.boneMeshMaterial);
            this.bones[id] = boneMesh;
            this.add(boneMesh);
        }
        return this.bones[id];
    }

    private updateBonePosition(fingerType: number, boneType: number, position: THREE.Vector3) {
        const id = `${fingerType},${boneType}`;
        if (this.bones[id]) {
            this.bones[id].position.copy(position);
        }
    }

    private updateFingerGeometry(fingerType: number, boneType: number, position: THREE.Vector3) {
        const fingerLine = this.fingers["finger" + fingerType];
        if (fingerLine) {
            const fingerGeometry = fingerLine.geometry as THREE.BufferGeometry;
            const positions = fingerGeometry.getAttribute('position') as THREE.Float32BufferAttribute || 
                new THREE.Float32BufferAttribute(new Float32Array((boneType + 1) * 3), 3);
            positions.setXYZ(boneType, position.x, position.y, position.z);
            fingerGeometry.setAttribute('position', positions);
        }
    }

    public update(canvas: HTMLCanvasElement, hand: Leap.Hand) {
        hand.fingers.forEach((finger) => {
            this.addFinger(finger.type);
            finger.bones.forEach((bone) => {
                this.addBone(finger.type, bone.type);
                const {x, y} = mapLeapToThreePosition(canvas, bone.center());
                // @todo Don't create a new Vector3 every frame? Is this called every frame?
                const position = new THREE.Vector3(x, y, 300); // z is set to 300 to keep it close to the camera
                this.updateBonePosition(finger.type, bone.type, position);
                this.updateFingerGeometry(finger.type, bone.type, position);
            });
        });
    }
}